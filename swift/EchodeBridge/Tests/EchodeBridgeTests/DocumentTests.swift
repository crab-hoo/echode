import CEchodeFfi
import XCTest
@testable import EchodeBridge

final class DocumentTests: XCTestCase {
    func testVisibleElementLayoutMatchesTheGeneratedCStruct() {
        XCTAssertEqual(MemoryLayout<VisibleElement>.size, 72)
        XCTAssertEqual(MemoryLayout<VisibleElement>.stride, 72)
        XCTAssertEqual(MemoryLayout<VisibleElement>.alignment, 4)
        XCTAssertEqual(MemoryLayout<PianoNoteElement>.size, 104)
        XCTAssertEqual(MemoryLayout<PianoNoteElement>.stride, 104)
        XCTAssertEqual(MemoryLayout<PianoNoteElement>.alignment, 8)
        XCTAssertEqual(MemoryLayout<TransportPosition>.size, 16)
    }

    func testSessionCommandsKeepProjectUndoAndOwnedStateIndependent() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("echode-audio-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: directory) }
        let document = try Document(creating: directory.path)
        _ = try await document.perform(#"{"type":"track.add","name":"keep"}"#)
        let state = try await document.audio(.locate(tick: 705_600))
        XCTAssertEqual(state.tick, 705_600)
        XCTAssertFalse(state.playing)
        XCTAssertTrue(state.devices.isEmpty)
        let feed = document.playbackFeed
        XCTAssertEqual(feed.position.tick, 705_600)
        let undone = try await document.undo()
        XCTAssertEqual(undone.trackCount, 0)
        let retained = try await document.snapshot()
        _ = try await document.audio(.stop)
        XCTAssertEqual(try retained.audioState().tick, 705_600)
        do {
            _ = try await document.audio(.locate(tick: -1))
            XCTFail("negative seek")
        } catch {}
        document.shutdown()
        XCTAssertFalse(feed.position.playing)
        do {
            _ = try await document.audio(.status)
            XCTFail("stopped session accepted a command")
        } catch let error as BridgeError { XCTAssertTrue(error.message.contains("stopped")) }
    }
    func testAnAuditionReportsAnUnreadableFileThroughTheSnapshot() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("echode-audition-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: directory) }
        let document = try Document(creating: directory.path)
        // Decoding fails before any output device is asked for.
        let missing = FileManager.default.temporaryDirectory.appendingPathComponent("echode-missing-\(UUID()).wav")
        document.audition(missing, origin: AuditionOrigin(input: .key, delayMicroseconds: 900))
        var status: AuditionStatus?
        for _ in 0..<1000 {
            status = try await document.audio(.status).audition
            if status?.phase == .failed { break }
            try await Task.sleep(for: .milliseconds(5))
        }
        XCTAssertEqual(status?.phase, .failed)
        XCTAssertEqual(status?.path, missing.path)
        XCTAssertNotNil(status?.error)
        XCTAssertEqual(status?.handled, status?.request)
        XCTAssertNil(status?.sampleRate)
        let failed = try XCTUnwrap(status?.history.last)
        XCTAssertEqual(failed.request, status?.request)
        XCTAssertEqual(failed.input, .key)
        XCTAssertEqual(failed.inputDelayUs, 900)
        XCTAssertEqual(failed.phase, .failed)
        XCTAssertTrue(failed.settled)
        XCTAssertNotNil(failed.pickedUs)
        XCTAssertNil(failed.handedUs)
        document.audition(nil)
        let stopped = try await document.audio(.status).audition
        XCTAssertEqual(stopped?.phase, .stopped)
        XCTAssertEqual(stopped?.request, (status?.request ?? 0) + 1)
        XCTAssertEqual(stopped?.history.last?.input, .other)
        XCTAssertNil(stopped?.history.last?.inputDelayUs)
        document.shutdown()
    }
    func testAddingATrackCompletesOnceAndSurvivesClose() async throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("echode-bridge-\(UUID().uuidString)", isDirectory: true)
        let document = try Document(creating: directory.path)
        let snapshot = try await document.perform(#"{"type":"track.add","name":"piano"}"#)
        XCTAssertEqual(snapshot.trackCount, 1)
        let track = try XCTUnwrap(snapshot.trackID(at: 0))
        let placed = try await document.perform(
            #"{"type":"note.insert","track":"\#(track.uuidString)","pitch":60,"velocity":100,"start":0,"length":705600}"#
        )
        let visible = placed.visibleElements(
            originTick: 0, widthTicks: 705_600 * 4, originTrack: 0, heightTracks: 2,
            pixelsPerTick: 1, pixelsPerTrack: 20
        )
        XCTAssertEqual(visible.dropped, 0)
        XCTAssertTrue(visible.elements.contains { $0.role == .note && $0.id.note != nil })
        document.shutdown()
        do {
            _ = try await document.perform(#"{"type":"track.add","name":"late"}"#)
            XCTFail("a stopped document accepted a command")
        } catch let error as BridgeError {
            XCTAssertTrue(error.message.contains("stopped"))
        }
        XCTAssertEqual(snapshot.trackCount, 1)
        try? FileManager.default.removeItem(at: directory)
    }
}
