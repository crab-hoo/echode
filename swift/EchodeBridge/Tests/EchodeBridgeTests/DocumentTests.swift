import CEchodeFfi
import Foundation
import Testing

@testable import EchodeBridge

struct DocumentTests {
    @Test func visibleElementLayoutMatchesTheGeneratedCStruct() {
        #expect(MemoryLayout<VisibleElement>.size == 72)
        #expect(MemoryLayout<VisibleElement>.stride == 72)
        #expect(MemoryLayout<VisibleElement>.alignment == 4)
        #expect(MemoryLayout<PianoNoteElement>.size == 104)
        #expect(MemoryLayout<PianoNoteElement>.stride == 104)
        #expect(MemoryLayout<PianoNoteElement>.alignment == 8)
        #expect(MemoryLayout<TransportPosition>.size == 16)
    }

    @Test func sessionCommandsKeepProjectUndoAndOwnedStateIndependent() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("echode-audio-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: directory) }
        let document = try Document(creating: directory.path)
        _ = try await document.perform(#"{"type":"track.add","name":"keep"}"#)
        let state = try await document.audio(.locate(tick: 705_600))
        #expect(state.tick == 705_600)
        #expect(!state.playing)
        #expect(state.devices.isEmpty)
        let feed = document.playbackFeed
        #expect(feed.position.tick == 705_600)
        let undone = try await document.undo()
        #expect(undone.trackCount == 0)
        let retained = try await document.snapshot()
        _ = try await document.audio(.stop)
        #expect(try retained.audioState().tick == 705_600)
        do {
            _ = try await document.audio(.locate(tick: -1))
            Issue.record("negative seek")
        } catch {}
        document.shutdown()
        #expect(!feed.position.playing)
        do {
            _ = try await document.audio(.status)
            Issue.record("stopped session accepted a command")
        } catch let error as BridgeError { #expect(error.message.contains("stopped")) }
    }
    @Test func anAuditionReportsAnUnreadableFileThroughTheSnapshot() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(
            "echode-audition-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: directory) }
        let document = try Document(creating: directory.path)
        // Decoding fails before any output device is asked for.
        #expect(document.configureAudition(visible: false, gain: 0.25))
        #expect(!document.configureAudition(visible: false, gain: .nan))
        let missing = FileManager.default.temporaryDirectory.appendingPathComponent("echode-missing-\(UUID()).wav")
        document.audition(missing, from: 137, origin: AuditionOrigin(input: .key, delayMicroseconds: 900))
        var status: AuditionStatus?
        for _ in 0..<1000 {
            status = try await document.audio(.status).audition
            if status?.phase == .failed { break }
            try await Task.sleep(for: .milliseconds(5))
        }
        #expect(status?.phase == .failed)
        #expect(status?.path == missing.path)
        #expect(status?.error != nil)
        #expect(status?.handled == status?.request)
        #expect(status?.sampleRate == nil)
        let failed = try #require(status?.history.last)
        #expect(failed.request == status?.request)
        #expect(failed.input == .key)
        #expect(failed.inputDelayUs == 900)
        #expect(failed.startFrame == 137)
        #expect(status?.settings?.gain == 0.25)
        #expect(status?.settings?.visible == false)
        #expect(failed.phase == .failed)
        #expect(failed.settled)
        #expect(failed.pickedUs != nil)
        #expect(failed.handedUs == nil)
        document.audition(nil)
        let stopped = try await document.audio(.status).audition
        #expect(stopped?.phase == .stopped)
        #expect(stopped?.request == (status?.request ?? 0) + 1)
        #expect(stopped?.history.last?.input == .other)
        #expect(stopped?.history.last?.inputDelayUs == nil)
        document.shutdown()
    }
    @Test func addingATrackCompletesOnceAndSurvivesClose() async throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("echode-bridge-\(UUID().uuidString)", isDirectory: true)
        let document = try Document(creating: directory.path)
        let snapshot = try await document.perform(#"{"type":"track.add","name":"piano"}"#)
        #expect(snapshot.trackCount == 1)
        let track = try #require(snapshot.trackID(at: 0))
        let placed = try await document.perform(
            #"{"type":"note.insert","track":"\#(track.uuidString)","pitch":60,"velocity":100,"start":0,"length":705600}"#
        )
        let visible = placed.visibleElements(
            originTick: 0, widthTicks: 705_600 * 4, originTrack: 0, heightTracks: 2,
            pixelsPerTick: 1, pixelsPerTrack: 20
        )
        #expect(visible.dropped == 0)
        #expect(visible.elements.contains { $0.role == .note && $0.id.note != nil })
        document.shutdown()
        do {
            _ = try await document.perform(#"{"type":"track.add","name":"late"}"#)
            Issue.record("a stopped document accepted a command")
        } catch let error as BridgeError {
            #expect(error.message.contains("stopped"))
        }
        #expect(snapshot.trackCount == 1)
        try? FileManager.default.removeItem(at: directory)
    }
}
