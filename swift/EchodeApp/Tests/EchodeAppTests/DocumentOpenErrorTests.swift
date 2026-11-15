import EchodeBridge
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

    @Test func failedSnapshotShutsDownOpenedDocumentOffMainActorAndReleasesLock() async throws {
        let current = FileManager.default.temporaryDirectory.appendingPathComponent("echode-current-\(UUID()).echode")
        let second = FileManager.default.temporaryDirectory.appendingPathComponent("echode-second-\(UUID()).echode")
        defer {
            removeTestFile(current)
            removeTestFile(second)
        }

        let seed = ProjectController()
        await seed.open(second, creating: true)
        await seed.addTrack(name: "second")
        await seed.close()

        let controller = ProjectController()
        let recorder = ShutdownRecorder()
        let snapshotGate = FailureOnce()
        controller.endDocument = { document in
            let onMain = await Task.detached {
                shutDownAndCheckMain(document)
            }.value
            await recorder.record(onMainThread: onMain)
        }
        controller.readSnapshot = { document in
            if await snapshotGate.takeFailure() { throw SnapshotFailure() }
            return try await document.snapshot()
        }

        await controller.open(current, creating: true)
        await controller.addTrack(name: "keep")
        let identity = try #require(controller.documentID)

        await snapshotGate.arm()
        await controller.open(second)
        #expect(controller.errorMessage?.contains("injected snapshot failure") == true)
        #expect(controller.documentID == identity)
        #expect(controller.projectURL == current)
        #expect(controller.tracks.map(\.name) == ["keep"])
        #expect(!controller.busy)
        #expect(await recorder.values() == [false], "aborted document must shut down off MainActor")

        controller.errorMessage = nil
        await controller.open(second)
        #expect(controller.errorMessage == nil)
        #expect(controller.projectURL?.resolvingSymlinksInPath() == second.resolvingSymlinksInPath())
        #expect(controller.documentID != identity)
        #expect(controller.tracks.map(\.name) == ["second"])
        #expect(await recorder.values() == [false, false])
        await controller.close()
    }
}

private struct SnapshotFailure: LocalizedError {
    var errorDescription: String? { "injected snapshot failure" }
}

private func shutDownAndCheckMain(_ document: Document) -> Bool {
    let onMain = Thread.isMainThread
    document.shutdown()
    return onMain
}

private actor ShutdownRecorder {
    private var entries: [Bool] = []
    func record(onMainThread: Bool) { entries.append(onMainThread) }
    func values() -> [Bool] { entries }
}

private actor FailureOnce {
    private var armed = false
    func arm() { armed = true }
    func takeFailure() -> Bool {
        guard armed else { return false }
        armed = false
        return true
    }
}
