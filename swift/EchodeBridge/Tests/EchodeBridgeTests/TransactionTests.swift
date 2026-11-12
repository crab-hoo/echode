import Foundation
import Testing

@testable import EchodeBridge

struct TransactionTests {
    @Test func actionArrayCommitsOnceAndRollsBackOnFailure() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-transaction-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let document = try Document(creating: url.path)
        defer { document.shutdown() }
        let added = try await document.perform(
            """
            [{"type":"track.add","name":"first"},{"type":"track.add","name":"second"}]
            """)
        #expect(added.trackCount == 2)
        let undone = try await document.undo()
        #expect(undone.trackCount == 0)
        let redone = try await document.redo()
        #expect(redone.trackID(at: 0) == added.trackID(at: 0))
        #expect(redone.trackID(at: 1) == added.trackID(at: 1))
        _ = try await document.undo()
        for invalid in [
            #"{"type":"device.add","track":"00000000-0000-0000-0000-000000000000","plugin":"org.example.fx"}"#,
            #"{"type":"unknown.action"}"#,
        ] {
            do {
                _ = try await document.perform("[{\"type\":\"track.add\",\"name\":\"rollback\"},\(invalid)]")
                Issue.record("a rejected element must roll back the whole array")
            } catch {}
            let unchanged = try await document.snapshot()
            #expect(unchanged.trackCount == 0)
        }
        _ = try await document.perform("[]")
        let retainedRedo = try await document.redo()
        #expect(retainedRedo.trackID(at: 0) == added.trackID(at: 0))
        #expect(retainedRedo.trackCount == 2)
    }
}
