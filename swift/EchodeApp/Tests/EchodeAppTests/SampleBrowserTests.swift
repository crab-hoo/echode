import AppKit
import EchodeBridge
import Foundation
import Synchronization
import Testing
@testable import EchodeAppCore

@MainActor
struct SampleBrowserTests {
    @Test func contractRegisteredSampleSearchCoversAllRootsAndSkipsPackagesAndLinks() async throws {
        let root = FileManager.default.temporaryDirectory.resolvingSymlinksInPath().appendingPathComponent("echode-library-\(UUID())")
        defer { removeTestFile(root) }
        let first = root.appendingPathComponent("first"), second = root.appendingPathComponent("second")
        let nested = first.appendingPathComponent("nested"), package = first.appendingPathComponent("Song.echode")
        for folder in [nested, package, second] { try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true) }
        for path in [nested.appendingPathComponent("Kick 2.WAV"), second.appendingPathComponent("kick 10.flac"),
                     first.appendingPathComponent("notes.txt"), first.appendingPathComponent(".hidden-kick.wav"), package.appendingPathComponent("kick.wav")] {
            try Data().write(to: path)
        }
        try FileManager.default.createSymbolicLink(at: nested.appendingPathComponent("cycle"), withDestinationURL: first)
        try FileManager.default.createSymbolicLink(at: first.appendingPathComponent("outside"), withDestinationURL: second)
        let name = "echode-library-\(UUID())"
        let defaults = try #require(UserDefaults(suiteName: name))
        defer { defaults.removePersistentDomain(forName: name) }
        let browser = FileBrowserState(defaults: defaults)
        await browser.register(first)
        browser.query = "kick"
        let kick = try #require(browser.roots.first).appendingPathComponent("nested/Kick 2.WAV")
        #expect(browser.filtered.map(\.url) == [kick])
        await browser.register(second)
        await browser.register(nested) // Overlapping roots cannot duplicate results.
        await browser.register(first)
        #expect(browser.roots.count == 3)
        browser.query = "KICK"
        let other = try #require(browser.roots.first { $0.lastPathComponent == "second" })
        let expected = [kick, other.appendingPathComponent("kick 10.flac")]
        #expect(browser.filtered.map(\.url) == expected)
        #expect(browser.libraryError == nil && !browser.indexing)
        let restored = FileBrowserState(defaults: defaults)
        await restored.prepare()
        restored.query = "kick"
        #expect(restored.filtered.map(\.url) == expected)
        await restored.load(second)
        #expect(restored.query.isEmpty && restored.filtered.count == 1)
        restored.query = "kick"
        await restored.refresh()
        #expect(restored.query == "kick" && restored.filtered.map(\.url) == expected)
        await restored.removeRoot(second)
        #expect(restored.filtered.map(\.url) == [expected[0]])
        try FileManager.default.removeItem(at: nested)
        await restored.refreshLibrary()
        #expect(restored.libraryError != nil && restored.libraryFiles.map(\.url.lastPathComponent) == ["Song.echode"] && !restored.indexing)
        await restored.register(root.appendingPathComponent("missing"))
        #expect(restored.roots.count == 2 && restored.error != nil)
    }

    @Test func contractSampleInsertionCapturesItsTargetAndSurvivesUndoAndReopen() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-sample-target-\(UUID())")
        defer { removeTestFile(root) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let wav = root.appendingPathComponent("tone.wav"), project = root.appendingPathComponent("Song.echode")
        try wavMono(rate: 48_000, frames: 4_800).write(to: wav)
        let controller = ProjectController()
        let state = WorkspaceState(defaults: .init())
        let session = WorkspaceSession(controller: controller, state: state)
        await controller.open(project, creating: true)
        await controller.addTrack(name: "first")
        let first = try #require(controller.selection)
        await controller.addTrack(name: "second")
        let second = try #require(controller.selection)
        controller.selection = first
        await controller.locate(beat: 1.2345)
        let tick = try #require(controller.playback?.position.tick)
        let undo = controller.undoCount
        let pending = try #require(session.importSample(BrowserFile(url: wav, directory: false)))
        controller.selection = second // Change before the queued import can begin.
        #expect(await pending.value)
        #expect(controller.selection == second && controller.undoCount == undo + 1)
        let clip = try #require(controller.snapshot?.visibleElements(originTick: 0, widthTicks: 20_000_000,
            originTrack: 0, heightTracks: 4, pixelsPerTick: 0.001, pixelsPerTrack: 40).elements.first { $0.role == .clip })
        let placement = try #require(clip.id.placement)
        #expect(clip.id.track == first && controller.snapshot?.clipMix(placement)?.start == tick)
        // A placed sample keeps its recorded edges; there is no automatic fade (ADR 0161).
        #expect(controller.snapshot?.clipMix(placement)?.fadeIn == 0 && controller.snapshot?.clipMix(placement)?.fadeOut == 0)
        await controller.undo()
        #expect(controller.snapshot?.clipMix(placement) == nil)
        await controller.redo()
        #expect(controller.snapshot?.clipMix(placement)?.start == tick)
        let stale = try #require(controller.captureAudioImport(wav, on: second))
        await controller.close(); await controller.open(project)
        #expect(controller.snapshot?.clipMix(placement)?.track == first)
        #expect(controller.snapshot?.clipMix(placement)?.start == tick)
        #expect(controller.snapshot?.clipMix(placement)?.fadeIn == 0 && controller.snapshot?.clipMix(placement)?.fadeOut == 0)
        #expect(!(await controller.importAudio(stale)))
        // Reopening has no undo history; a newly added target can be removed by undo.
        await controller.addTrack(name: "temporary")
        let absent = try #require(controller.captureAudioImport(wav))
        await controller.undo()
        #expect(!(await controller.importAudio(absent)))
        #expect(controller.captureAudioImport(wav, on: UUID()) == nil)
        #expect(session.importSample(BrowserFile(url: root, directory: true)) == nil)
        #expect(controller.undoCount == 0)
        await controller.close()
    }

    @Test func contractBrowserSelectionAuditionsOnlySamplesWithoutTheEditQueue() async throws {
        let root = FileManager.default.temporaryDirectory.resolvingSymlinksInPath().appendingPathComponent("echode-audition-\(UUID())")
        defer { removeTestFile(root) }
        let folder = root.appendingPathComponent("samples")
        try FileManager.default.createDirectory(at: folder.appendingPathComponent("loops"), withIntermediateDirectories: true)
        for name in ["kick.wav", "snare.aif", "melody.mid"] {
            try wavMono(rate: 48_000, frames: 480).write(to: folder.appendingPathComponent(name))
        }
        let requests = Mutex<[URL?]>([])
        let controller = ProjectController(sendAudition: { _, url, _ in requests.withLock { $0.append(url) } })
        let state = WorkspaceState(defaults: .init())
        let session = WorkspaceSession(controller: controller, state: state)
        await state.browser.load(folder)
        let rows = Dictionary(uniqueKeysWithValues: state.browser.filtered.map { ($0.url.lastPathComponent, $0.url) })
        let kick = try #require(rows["kick.wav"]), snare = try #require(rows["snare.aif"])
        // Nothing plays without a document.
        #expect(session.auditionSelection(kick) == nil)
        #expect(requests.withLock { $0.isEmpty })
        await controller.open(root.appendingPathComponent("Song.echode"), creating: true)
        // The request is synchronous; it is not an edit and never waits for one.
        #expect(session.auditionSelection(kick) == kick)
        #expect(session.auditionSelection(snare) == snare)
        let loops = try #require(rows["loops"]), notes = try #require(rows["melody.mid"])
        #expect(session.auditionSelection(loops) == nil)
        #expect(session.auditionSelection(notes) == nil)
        #expect(session.auditionSelection(nil) == nil)
        // A path that is not a shown row cannot start a sound.
        #expect(session.auditionSelection(root.appendingPathComponent("elsewhere.wav")) == nil)
        #expect(requests.withLock { $0 } == [kick, snare, nil, nil, nil, nil])
        #expect(controller.undoCount == 0)
        await controller.close()
    }

    @Test func contractBrowserClickSelectsAndFocusesAtOnceAndOpensOnlyOnDoubleClick() async throws {
        let root = FileManager.default.temporaryDirectory.resolvingSymlinksInPath().appendingPathComponent("echode-browser-click-\(UUID())")
        defer { removeTestFile(root) }
        let folder = root.appendingPathComponent("samples")
        try FileManager.default.createDirectory(at: folder.appendingPathComponent("loops"), withIntermediateDirectories: true)
        for name in ["kick.wav", "snare.wav"] { try wavMono(rate: 48_000, frames: 480).write(to: folder.appendingPathComponent(name)) }
        let requests = Mutex<[(URL?, AuditionOrigin)]>([])
        let controller = ProjectController(sendAudition: { _, url, origin in requests.withLock { $0.append((url, origin)) } })
        let state = WorkspaceState(defaults: .init())
        let session = WorkspaceSession(controller: controller, state: state)
        await controller.open(root.appendingPathComponent("Song.echode"), creating: true)
        await controller.addTrack(name: "audio")
        let browser = state.browser
        await browser.load(folder)
        // The view wires the list to the session the same way.
        let list = FileListView()
        let window = NSWindow(contentRect: CGRect(x: 0, y: 0, width: 240, height: 300), styleMask: [.titled], backing: .buffered, defer: false)
        window.contentView = list
        var pending: Task<Void, Never>?
        list.onSelect = { session.chooseInBrowser($0, origin: $1) }
        list.onOpen = { pending = session.openInBrowser($0) }
        func refresh() {
            list.show(browser.filtered, searching: browser.searchingLibrary, selection: browser.selection)
            window.layoutIfNeeded()
        }
        refresh()
        #expect(browser.filtered.map(\.url.lastPathComponent) == ["loops", "kick.wav", "snare.wav"])
        let table = list.table
        func point(_ row: Int, below: Bool = false) -> CGPoint {
            let rect = table.rect(ofRow: row)
            return table.convert(CGPoint(x: rect.midX, y: below ? rect.maxY + 40 : rect.midY), to: nil)
        }
        func click(_ location: CGPoint, count: Int) throws {
            table.mouseDown(with: try #require(NSEvent.mouseEvent(with: .leftMouseDown, location: location, modifierFlags: [],
                timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber, context: nil,
                eventNumber: count, clickCount: count, pressure: 1)))
        }
        func key(_ code: UInt16, _ characters: String) throws {
            table.keyDown(with: try #require(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [],
                timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber, context: nil,
                characters: characters, charactersIgnoringModifiers: characters, isARepeat: false, keyCode: code)))
        }
        func asked() -> [URL?] { requests.withLock { $0.map(\.0) } }
        let kick = browser.filtered[1].url, snare = browser.filtered[2].url
        let undo = controller.undoCount

        // One click selects, takes the keyboard and asks for the sound before it returns.
        try click(point(1), count: 1)
        #expect(browser.selection == kick && table.selectedRow == 1)
        #expect(window.firstResponder === table)
        #expect(asked() == [kick])
        let origin = try #require(requests.withLock { $0.last?.1 })
        #expect(origin.input == .click && origin.delayMicroseconds < 1_000_000)
        #expect(pending == nil && controller.undoCount == undo)
        // The second click of a double click places it, with no second request.
        try click(point(1), count: 2)
        await pending?.value
        #expect(controller.undoCount == undo + 1 && asked() == [kick])

        // Arrow keys move the selection as key input. Return places.
        pending = nil
        try key(125, "\u{F701}")
        #expect(browser.selection == snare && asked() == [kick, snare])
        #expect(requests.withLock { $0.last?.1.input } == .key)
        await controller.locate(beat: 4) // Placements on one track cannot overlap.
        try key(36, "\r")
        await pending?.value
        #expect(controller.undoCount == undo + 2 && asked() == [kick, snare])
        // Typing a name selects its row as key input.
        #expect(list.tableView(table, typeSelectStringFor: nil, row: 1) == "kick.wav")

        // The browser moving its own selection after a search is silent.
        browser.query = "kick"
        refresh()
        #expect(browser.selection == kick && table.selectedRow == 0)
        browser.query = ""
        refresh()
        #expect(table.selectedRow == 1 && asked() == [kick, snare])

        // A click below the rows clears the selection and stops the sound.
        try click(point(2, below: true), count: 1)
        #expect(browser.selection == nil && table.selectedRow == -1)
        #expect(asked() == [kick, snare, nil])
        // A folder stops it too, and entering it clears the selection silently.
        try click(point(0), count: 1)
        try click(point(0), count: 2)
        await pending?.value
        refresh()
        #expect(browser.folder?.lastPathComponent == "loops" && browser.selection == nil)
        #expect(asked() == [kick, snare, nil, nil])
        #expect(controller.undoCount == undo + 2)
        await controller.close()
    }

    @Test func auditionLogWritesEachRequestOnceWhenNothingMoreWillBeRecorded() throws {
        func status(_ newest: UInt64, _ entries: [String]) throws -> AuditionStatus {
            let json = #"{"request":\#(newest),"handled":\#(newest),"phase":"sounding","history":[\#(entries.joined(separator: ","))]}"#
            return try JSONDecoder().decode(AuditionStatus.self, from: Data(json.utf8))
        }
        func entry(_ request: UInt64, _ phase: String, heard: Bool = false) -> String {
            #"{"request":\#(request),"input":"click","requested_at_ms":1,"phase":"\#(phase)","opened_output":false\#(heard ? #","heard_us":9"# : "")}"#
        }
        let log = AuditionLog()
        var written: [UInt64] = []
        log.write = { written.append($0.request) }
        log.record(try status(2, [entry(1, "sounding", heard: true), entry(2, "loading")]))
        log.record(try status(2, [entry(1, "sounding", heard: true), entry(2, "loading")]))
        #expect(written == [1])
        // The newest waits until it is heard or ends.
        log.record(try status(2, [entry(1, "sounding", heard: true), entry(2, "sounding")]))
        #expect(written == [1])
        // Replaced before it was heard: no later stage can come.
        log.record(try status(3, [entry(2, "sounding"), entry(3, "failed")]))
        #expect(written == [1, 2, 3])
        // A closed document resets the numbering for the next one.
        log.record(nil)
        log.record(try status(1, [entry(1, "stopped")]))
        #expect(written == [1, 2, 3, 1])

        // The app's file holds one JSON line per entry and keeps one older file.
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("echode-audition-log-\(UUID())")
        defer { removeTestFile(folder) }
        let file = LineLog(url: folder.appendingPathComponent("audition.ndjson"), limit: 250)
        let kept = AuditionLog()
        kept.keep(in: file)
        kept.record(try status(2, [entry(1, "sounding", heard: true), entry(2, "failed")]))
        file.flush()
        let lines = try String(contentsOf: file.url, encoding: .utf8).split(separator: "\n")
        #expect(lines.count == 2)
        let first = try #require(JSONSerialization.jsonObject(with: Data(lines[0].utf8)) as? [String: Any])
        #expect(first["request"] as? Int == 1 && first["heard_us"] as? Int == 9 && first["input"] as? String == "click")
        kept.record(try status(4, [entry(3, "stopped"), entry(4, "stopped")]))
        file.flush()
        #expect(FileManager.default.fileExists(atPath: file.previous.path))
        let current = try String(contentsOf: file.url, encoding: .utf8) + String(contentsOf: file.previous, encoding: .utf8)
        #expect(current.split(separator: "\n").count == 4)
    }

    @Test func supersededLibraryScanCannotRestoreARemovedRoot() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-sample-race-\(UUID())")
        defer { removeTestFile(root) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        try Data().write(to: root.appendingPathComponent("sample.wav"))
        let browser = FileBrowserState()
        await browser.register(root)
        var started = false
        let older = Task { started = true; await browser.refreshLibrary() }
        while !started { await Task.yield() }
        await browser.removeRoot(try #require(browser.roots.first))
        await older.value
        #expect(browser.roots.isEmpty && browser.libraryFiles.isEmpty && !browser.indexing)
    }

    @Test func importUsesTheLivePlayheadInsteadOfTheLastStatusPoll() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-sample-\(UUID())")
        defer { removeTestFile(root) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let wav = root.appendingPathComponent("tone.wav")
        try wavMono(rate: 48_000, frames: 4_800).write(to: wav)
        // Keep the UI's last status stale while the product feed advances.
        // Locate and import do not open an audio device.
        let controller = ProjectController(sendAudio: { document, command in
            _ = try await document.audio(command)
            return AudioState()
        })
        await controller.open(root.appendingPathComponent("Song.echode"), creating: true)
        await controller.addTrack(name: "audio")
        await controller.locate(beat: 1.2345)
        let tick = try #require(controller.playback?.position.tick)
        #expect(tick > 0 && controller.audio.tick == 0)
        #expect(await controller.importAudio(wav))
        let snapshot = try #require(controller.snapshot)
        let elements = snapshot.visibleElements(originTick: 0, widthTicks: 20_000_000,
            originTrack: 0, heightTracks: 4, pixelsPerTick: 0.001, pixelsPerTrack: 40)
        let clip = try #require(elements.elements.first { $0.role == .clip })
        #expect(snapshot.clipMix(try #require(clip.id.placement))?.start == tick)
        await controller.close()
    }
}
