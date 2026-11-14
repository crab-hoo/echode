import Foundation
import Testing

@testable import EchodeBridge

struct DocumentOpenErrorTests {
    @Test func openingAnAlreadyOpenPackageReportsTheLockAndKeepsTheDocumentUsable() async throws {
        let directory = temporaryPackage()
        defer { try? FileManager.default.removeItem(at: directory) }
        let document = try Document(creating: directory.path)
        defer { document.shutdown() }

        do {
            _ = try Document(opening: directory.path)
            Issue.record("a second document acquired the package")
        } catch let error as BridgeError {
            #expect(error.message.contains("package is already open"))
        }
        let snapshot = try await document.perform(#"{"type":"track.add","name":"kept"}"#)
        #expect(snapshot.trackCount == 1)
    }

    @Test func creatingOverAnExistingDirectoryReportsTheReasonAndPreservesItsContents() throws {
        let directory = temporaryPackage()
        defer { try? FileManager.default.removeItem(at: directory) }
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let sentinel = directory.appendingPathComponent("keep.txt")
        try Data("untouched".utf8).write(to: sentinel)

        do {
            _ = try Document(creating: directory.path)
            Issue.record("creation replaced an existing directory")
        } catch let error as BridgeError {
            #expect(error.message.contains("already exists"))
            #expect(error.message.contains(directory.path))
        }
        #expect(try Data(contentsOf: sentinel) == Data("untouched".utf8))
    }

    @Test func openingAnInvalidPackageReportsTheUnderlyingParseError() throws {
        let directory = temporaryPackage()
        defer { try? FileManager.default.removeItem(at: directory) }
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        try Data("{".utf8).write(to: directory.appendingPathComponent("project.json"))

        do {
            _ = try Document(opening: directory.path)
            Issue.record("an invalid package was opened")
        } catch let error as BridgeError {
            #expect(error.message.contains("EOF while parsing an object"))
            #expect(error.message.contains("line 1 column 1"))
        }
    }

    private func temporaryPackage() -> URL {
        FileManager.default.temporaryDirectory.appendingPathComponent("echode-open-error-\(UUID()).echode")
    }
}
