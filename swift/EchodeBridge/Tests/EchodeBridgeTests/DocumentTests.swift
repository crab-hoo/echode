import XCTest
@testable import EchodeBridge

final class DocumentTests: XCTestCase {
    func testAddingATrackCompletesOnceAndSurvivesClose() async throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("echode-bridge-\(UUID().uuidString)", isDirectory: true)
        let document = try Document(creating: directory.path)
        let snapshot = try await document.perform(#"{"type":"track.add","name":"piano"}"#)
        XCTAssertEqual(snapshot.trackCount, 1)
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
