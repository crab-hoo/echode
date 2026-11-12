import Foundation
import XCTest

@testable import EchodeBridge

final class TransactionTests: XCTestCase {
    func testActionArrayCommitsOnceAndRollsBackOnFailure() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-transaction-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let document = try Document(creating: url.path)
        defer { document.shutdown() }
        let added = try await document.perform(
            """
            [{"type":"track.add","name":"first"},{"type":"track.add","name":"second"}]
            """)
        XCTAssertEqual(added.trackCount, 2)
        let undone = try await document.undo()
        XCTAssertEqual(undone.trackCount, 0)
        let redone = try await document.redo()
        XCTAssertEqual(redone.trackID(at: 0), added.trackID(at: 0))
        XCTAssertEqual(redone.trackID(at: 1), added.trackID(at: 1))
        _ = try await document.undo()
        for invalid in [
            #"{"type":"device.add","track":"00000000-0000-0000-0000-000000000000","plugin":"org.example.fx"}"#,
            #"{"type":"unknown.action"}"#,
        ] {
            do {
                _ = try await document.perform("[{\"type\":\"track.add\",\"name\":\"rollback\"},\(invalid)]")
                XCTFail("a rejected element must roll back the whole array")
            } catch {}
            let unchanged = try await document.snapshot()
            XCTAssertEqual(unchanged.trackCount, 0)
        }
        _ = try await document.perform("[]")
        let retainedRedo = try await document.redo()
        XCTAssertEqual(retainedRedo.trackID(at: 0), added.trackID(at: 0))
        XCTAssertEqual(retainedRedo.trackCount, 2)
    }
}
