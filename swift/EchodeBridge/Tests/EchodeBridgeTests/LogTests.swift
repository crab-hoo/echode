import CEchodeFfi
import XCTest

@testable import EchodeBridge

final class LogTests: XCTestCase {
    /// The app's records reach the core's file with their level and category,
    /// and its audition lines share the core's rotation rule (ADR 0322).
    func testAppRecordsReachTheInstalledFileAndAppendedLinesRotate() throws {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("echode-log-\(UUID())")
        defer { try? FileManager.default.removeItem(at: folder) }
        let file = folder.appendingPathComponent("echode.ndjson")
        EchodeLog.install(at: file)
        let marker = UUID().uuidString
        EchodeLog.write(.warning, "bridge", "record \(marker)")
        EchodeLog.flush()
        let lines = try String(contentsOf: file, encoding: .utf8).split(separator: "\n")
        let records = try lines.map {
            try XCTUnwrap(JSONSerialization.jsonObject(with: Data($0.utf8)) as? [String: Any])
        }
        let record = try XCTUnwrap(records.first { $0["message"] as? String == "record \(marker)" })
        XCTAssertEqual(record["level"] as? String, "warn")
        XCTAssertEqual(record["target"] as? String, "app::bridge")

        let audition = folder.appendingPathComponent("audition.ndjson")
        let line = Data("{\"request\":1}\n".utf8)
        XCTAssertTrue(EchodeLog.append(line, to: audition, limit: 20))
        XCTAssertTrue(EchodeLog.append(line, to: audition, limit: 20))
        XCTAssertEqual(try Data(contentsOf: audition), line)
        let previous = folder.appendingPathComponent("audition.1.ndjson")
        XCTAssertEqual(try Data(contentsOf: previous), line)
        XCTAssertEqual(EchodeLog.fileLimit, 1 << 20)
    }
}
