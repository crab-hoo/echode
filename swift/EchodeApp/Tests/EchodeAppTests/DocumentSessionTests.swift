import EchodeBridge
import Foundation
import Synchronization
import Testing
@testable import EchodeAppCore

/// Holds the first parameter value sent until `release`, so a request can
/// outlive the document it was sent to. Later values pass straight through.
private actor ParameterValueGate {
    private(set) var values: [(document: ObjectIdentifier, value: Double)] = []
    private var held: CheckedContinuation<Void, Never>?
    private var holding = true
    private let arrived = AsyncStream<Void>.makeStream()

    func arrive(_ document: Document, value: Double) async {
        values.append((ObjectIdentifier(document), value))
        arrived.continuation.yield()
        guard holding else { return }
        holding = false
        await withCheckedContinuation { held = $0 }
    }

    func waitForFirstValue() async { for await _ in arrived.stream { break } }
    func release() { held?.resume(); held = nil }
}

private actor AudioReplyGate {
    private var held: CheckedContinuation<Void, Never>?
    private let arrived = AsyncStream<Void>.makeStream()

    func hold() async {
        await withCheckedContinuation {
            held = $0
            arrived.continuation.yield()
        }
    }

    func waitUntilHeld() async { for await _ in arrived.stream { break } }
    func release() { held?.resume(); held = nil }
}

@MainActor
struct DocumentSessionTests {
    private func location(_ name: String) -> URL {
        FileManager.default.temporaryDirectory.appendingPathComponent("echode-session-\(name)-\(UUID()).echode")
    }

    /// A controller whose first parameter value stays in flight until released.
    private func heldParameters() -> (ProjectController, ParameterValueGate) {
        let plugin = FakeParameterPlugin()
        let gate = ParameterValueGate()
        let controller = ProjectController(sendDevice: { document, command in
            if case .set(_, let changes) = command, let change = changes.first {
                await gate.arrive(document, value: change.value)
            }
            return try await plugin.send(document, command)
        })
        return (controller, gate)
    }

    private func addListedDevice(_ controller: ProjectController, track name: String) async throws -> UUID {
        let document = try #require(controller.documentID)
        await controller.addTrack(name: name)
        let track = try #require(controller.selection)
        #expect(await controller.insertPlugin("dev.echode.test-params", on: track, documentID: document))
        let device = try #require(controller.deviceRows(on: track).first).id
        await controller.showDevices(on: track).value
        return device
    }

    @Test func contractADragInANewDocumentReachesThatDocument() async throws {
        let (first, second) = (location("drag-first"), location("drag-second"))
        defer { for url in [first, second] { try? FileManager.default.removeItem(at: url) } }
        let (controller, gate) = heldParameters()
        await controller.open(first, creating: true)
        let oldDevice = try await addListedDevice(controller, track: "old")
        #expect(controller.beginParameter(1, device: oldDevice))
        controller.changeParameter(1.5)
        await gate.waitForFirstValue()

        // The old document's reply is still out when the next document opens.
        await controller.open(second, creating: true)
        let opened = try #require(controller.document)
        let newDevice = try await addListedDevice(controller, track: "new")
        let undoSteps = controller.undoCount
        #expect(controller.beginParameter(1, device: newDevice))
        controller.changeParameter(0.5)
        await gate.release()
        await controller.finishParameter(commit: true)

        let last = try #require(await gate.values.last)
        #expect(last.value == 0.5 && last.document == ObjectIdentifier(opened))
        #expect(controller.errorMessage == nil)
        #expect(controller.parameterState(newDevice)?.parameter(1)?.value == 0.5)
        #expect(controller.undoCount == undoSteps + 1)
        await controller.close()
    }

    @Test func contractAReplyForTheOldDocumentDoesNotHoldTheNextOnesCommands() async throws {
        let (first, second) = (location("audio-first"), location("audio-second"))
        defer { for url in [first, second] { try? FileManager.default.removeItem(at: url) } }
        let gate = AudioReplyGate()
        let metronome = Mutex(false)
        let state = try JSONDecoder().decode(AudioState.self, from: Data("""
            {"playing":true,"running":true,"opening":false,"tick":123,"devices":[],"metronome":false}
            """.utf8))
        let controller = ProjectController(sendAudio: { document, command in
            switch command {
            case .play:
                await gate.hold()
                return state
            case .metronome:
                metronome.withLock { $0 = true }
                return try await document.snapshot().audioState()
            default:
                return try await document.audio(command)
            }
        })
        await controller.open(first, creating: true)
        let play = Task { await controller.togglePlayback() }
        await gate.waitUntilHeld()
        #expect(controller.audioBusy && controller.playbackRequested)

        await controller.open(second, creating: true)
        // The new document has sent nothing, so polling and playback intent are its own.
        #expect(!controller.audioBusy && !controller.playbackRequested)
        let toggle = Task { await controller.toggleMetronome() }
        var turns = 0
        while !metronome.withLock({ $0 }), turns < 10_000 {
            await Task.yield()
            turns += 1
        }
        #expect(metronome.withLock { $0 })

        await gate.release()
        await play.value
        await toggle.value
        // The late answer describes the document it was sent to.
        #expect(!controller.audio.playing && controller.audio.tick != state.tick)
        await controller.close()
    }

    @Test func contractARevisionRestoreStaysWithItsDocument() async throws {
        let url = location("restore")
        defer { try? FileManager.default.removeItem(at: url) }
        let (controller, gate) = heldParameters()
        await controller.open(url, creating: true)
        let device = try await addListedDevice(controller, track: "one")
        await controller.addTrack(name: "two")
        let revisions = await controller.savedRevisions()
        let older = try #require(revisions.dropLast().last)
        let document = try #require(controller.documentID)

        // The restore waits behind a parameter drag while the project is reopened.
        #expect(controller.beginParameter(1, device: device))
        controller.changeParameter(1.5)
        await gate.waitForFirstValue()
        let restore = Task { await controller.restoreRevision(older.id, documentID: document) }
        await Task.yield()
        await controller.close()
        await controller.open(url)
        let tracks = controller.tracks.map(\.name)
        #expect(tracks == ["one", "two"])
        await gate.release()

        #expect(await restore.value == nil)
        #expect(controller.tracks.map(\.name) == tracks && controller.undoCount == 0)
        await controller.close()
    }

    /// Reopening a project keeps its track, note and device IDs, so only the
    /// document ID tells an earlier input from a current one.
    @Test func contractEveryDocumentOperationRejectsAnEarlierDocument() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-session-stale-\(UUID())")
        defer { try? FileManager.default.removeItem(at: root) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let wav = root.appendingPathComponent("clip.wav"), url = root.appendingPathComponent("Project.echode")
        // This short fixture only supplies a clip to address, not a product buffer limit.
        try wavMono(rate: 48_000, frames: 4_800).write(to: wav)
        let plugin = FakeParameterPlugin()
        let controller = ProjectController(sendAudio: { _, _ in AudioState() }, sendDevice: { try await plugin.send($0, $1) })
        await controller.open(url, creating: true)
        await controller.addTrack(name: "one")
        let track = try #require(controller.selection), old = try #require(controller.documentID)
        #expect(await controller.importAudio(wav))
        let clip = try #require(controller.snapshot?.visibleElements(originTick: 0, widthTicks: 705_600 * 8, originTrack: 0, heightTracks: 2,
            pixelsPerTick: 200 / 705_600, pixelsPerTrack: 64).elements.first { $0.role == .clip }?.id.placement)
        #expect(await controller.insertPlugin("dev.echode.test-params", on: track, documentID: old))
        #expect(await controller.insertNote(pitch: 60, beat: 4, length: 1, on: track, documentID: old))
        let device = try #require(controller.deviceRows(on: track).first).id
        let note = try #require(controller.snapshot?.pianoNeighbor(track: track, current: nil))
        let revision = try #require(await controller.savedRevisions().last).id

        await controller.close()
        await controller.open(url)
        let current = try #require(controller.documentID)
        #expect(current != old)
        await controller.showDevices(on: track).value
        let contents = controller.snapshot
        let geometry = NoteGeometry(start: note.start + 1, length: note.length, pitch: note.pitch)

        #expect(!(await controller.insertPlugin("dev.echode.test-params", on: track, documentID: old)))
        #expect(!(await controller.insertPlugin("dev.echode.test-params", on: [track], documentID: old)))
        await controller.removeDevice(device, documentID: old)
        #expect(!(await controller.editDeviceChain(.bypass(device: device, bypassed: true), documentID: old)))
        #expect(!(await controller.dropFX(.copyDevice(device), on: track, documentID: old)))
        #expect(!(await controller.setPinned(1, device: device, pinned: true, documentID: old)))
        #expect(await controller.openPluginWindow(device, documentID: old) == nil)
        #expect(!controller.setDeviceCollapsed(device, collapsed: true, documentID: old))
        await controller.retryParameters(device, documentID: old).value
        #expect(!controller.beginParameter(1, device: device, documentID: old))
        #expect(!(await controller.setParameter(1, device: device, value: 0.5, documentID: old)))
        #expect(!(await controller.enterParameter("0.5", id: 1, device: device, documentID: old)))
        #expect(!(await controller.addPluginOpeningWindow("dev.echode.test-params", on: track, documentID: old)))
        #expect(!(await controller.insertNote(pitch: 62, beat: 0, length: 1, on: track, documentID: old)))
        #expect(!(await controller.editNote(note.id, geometry: geometry, documentID: old)))
        #expect(!(await controller.setNoteVelocity(note.id, velocity: 1, documentID: old)))
        #expect(!(await controller.deleteNote(note.id, documentID: old)))
        #expect(!(await controller.deleteNotes([note.id], documentID: old)))
        #expect(!(await controller.enqueuePianoStep(PianoStep(ids: [note.id], time: 1), documentID: old).value))
        #expect(!(await controller.setMixer(track, field: .gain, value: 0.5, documentID: old)))
        #expect(!controller.beginFader(track, field: .gain, documentID: old))
        #expect(!(await controller.setClipGain(clip, gain: 0.5, documentID: old)))
        #expect(!(await controller.setClipFades(clip, fadeIn: 1, fadeOut: 1, documentID: old)))
        #expect(!(await controller.moveClip(ClipMove(document: old, clip: clip, track: track, start: 1, copy: false))))
        #expect(!(await controller.importAudio(AudioImportRequest(url: wav, document: old, track: track, tick: 0))))
        #expect(!(await controller.importDroppedAudio(AudioFileDrop(document: old, url: wav, track: nil, tick: 0))))
        #expect(!(await controller.insertTrack(TrackInsertionRequest(document: old, after: nil, name: "stale"))))
        #expect(!(await controller.duplicateTracks(TrackDuplicationRequest(document: old, sources: [track]))))
        #expect(await controller.restoreRevision(revision, documentID: old) == nil)
        #expect(controller.captureMixerSwitch(track, field: .mute, document: old) == nil)
        #expect(controller.captureMixerValue(track, field: .gain, value: 0.5, document: old) == nil)
        #expect(controller.capturePluginInsertion("dev.echode.test-params", allSelected: false, document: old) == nil)
        #expect(!controller.openPiano(PianoTrackTarget(document: old, track: track)))
        controller.setPianoReference(PianoTrackTarget(document: old, track: track), visible: false)
        await controller.setLoopRange(try #require(LoopRange(start: 1, end: 2)), documentID: old)

        #expect(controller.snapshot === contents && controller.undoCount == 0 && controller.redoCount == 0)
        #expect(controller.errorMessage == nil && controller.loopRange == .initial && controller.hiddenPianoTracks.isEmpty)
        #expect(controller.collapsedDevices.isEmpty && controller.faderPreview == nil)
        // The fixture itself is valid: the same request for the open document applies.
        #expect(await controller.setMixer(track, field: .gain, value: 0.5, documentID: current))
        #expect(controller.undoCount == 1)
        await controller.close()
    }

    @Test func closingRemovesTheDocumentAndItsContentsTogether() async throws {
        let url = location("close")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "one")
        let closing = Task { await controller.close() }
        while controller.documentID != nil { await Task.yield() }
        // Views rendered during shutdown never pair old contents with no document.
        #expect(controller.snapshot == nil && controller.tracks.isEmpty && controller.projectURL == nil)
        #expect(controller.busy)
        await closing.value
        #expect(!controller.busy)
    }
}
