import Foundation
import Testing

@testable import EchodeAppCore

@MainActor
struct DocumentOpenErrorTests {
    @Test func failedAcquisitionsExposeTheirCauseWithoutReplacingTheCurrentDocument() async throws {
        let current = FileManager.default.temporaryDirectory.appendingPathComponent("echode-current-\(UUID()).echode")
        let broken = FileManager.default.temporaryDirectory.appendingPathComponent("echode-broken-\(UUID()).echode")
        defer {
            removeTestFile(current)
            removeTestFile(broken)
        }
        try FileManager.default.createDirectory(at: broken, withIntermediateDirectories: true)
        try Data("{".utf8).write(to: broken.appendingPathComponent("project.json"))

        let controller = ProjectController()
        await controller.open(current, creating: true)
        await controller.addTrack(name: "keep")
        let identity = try #require(controller.documentID)

        await controller.open(broken)
        #expect(controller.errorMessage?.contains("EOF while parsing an object") == true)
        #expect(controller.errorMessage?.contains(broken.path) == true)
        #expect(controller.documentID == identity)
        #expect(controller.projectURL == current)
        #expect(controller.tracks.map(\.name) == ["keep"])
        #expect(!controller.busy)

        controller.errorMessage = nil
        await controller.open(current, creating: true)
        #expect(controller.errorMessage?.contains("package already exists") == true)
        #expect(controller.documentID == identity)
        #expect(!controller.busy)
        await controller.addTrack(name: "still editable")
        #expect(controller.tracks.map(\.name) == ["keep", "still editable"])
        await controller.close()
    }
}
