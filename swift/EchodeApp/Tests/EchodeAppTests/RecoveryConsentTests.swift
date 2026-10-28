import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

@MainActor
struct RecoveryConsentTests {
    @Test func contractRecoveryWarningPrecedesDocumentPublicationAndEditing() async throws {
        let fixture = try await damagedProject()
        defer { try? FileManager.default.removeItem(at: fixture.url) }
        let previous = FileManager.default.temporaryDirectory.appendingPathComponent("echode-before-recovery-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: previous) }
        let prepares = PrepareCalls()
        let controller = ProjectController(sendAudio: { _, command in await prepares.receive(command) })
        await controller.open(previous, creating: true)
        await controller.addTrack(name: "previous document")
        let previousID = controller.documentID
        #expect(controller.recoveryPrompt == nil)
        controller.preparesOutput = true
        let original = try Data(contentsOf: fixture.url.appendingPathComponent("project.json"))
        let journal = try Data(contentsOf: fixture.url.appendingPathComponent("journal.jsonl"))

        let opening = Task { await controller.open(fixture.url) }
        try await waitForPrompt(controller, opening: opening)
        let prompt = try #require(controller.recoveryPrompt)
        #expect(prompt.notice.tip == fixture.tip && prompt.notice.base == fixture.base)
        #expect(prompt.message.contains(fixture.tip) && prompt.message.contains(fixture.base))
        #expect(controller.busy, "recovery consent must keep editing locked")
        #expect(await prepares.count == 0, "unconfirmed recovery must not prewarm output")
        #expect(controller.documentID == previousID, "unconfirmed recovery must not replace the document")
        #expect(controller.tracks.map(\.name) == ["previous document"])
        controller.respondToRecovery(accept: false)
        await opening.value
        #expect(!controller.busy && controller.recoveryPrompt == nil)
        #expect(controller.documentID == previousID)
        #expect(try Data(contentsOf: fixture.url.appendingPathComponent("project.json")) == original)
        #expect(try Data(contentsOf: fixture.url.appendingPathComponent("journal.jsonl")) == journal)
        #expect(try tip(fixture.url) == fixture.tip)

        let accepted = Task { await controller.open(fixture.url) }
        try await waitForPrompt(controller, opening: accepted)
        controller.respondToRecovery(prompt, accept: true)
        #expect(controller.recoveryPrompt != nil && controller.busy, "a stale alert must not accept another open")
        controller.respondToRecovery(accept: true)
        await accepted.value
        #expect(controller.errorMessage == nil)
        #expect(controller.documentID != previousID)
        #expect(controller.projectURL == fixture.url && controller.tracks.map(\.name) == ["A"])
        #expect(controller.undoCount == 0 && controller.redoCount == 0)
        #expect(try controller.snapshot?.historyReport().recovery == nil)
        #expect(try tip(fixture.url) != fixture.tip)
        await controller.addTrack(name: "confirmed")
        #expect(controller.undoCount == 1)
        await controller.close()
        await controller.open(fixture.url)
        #expect(controller.recoveryPrompt == nil && controller.errorMessage == nil)
        #expect(controller.tracks.map(\.name) == ["A", "confirmed"])
        await controller.close()
    }

    @Test func cancellingOpenReleasesConsentAndPackageOwnership() async throws {
        let fixture = try await damagedProject()
        defer { try? FileManager.default.removeItem(at: fixture.url) }
        let controller = ProjectController()
        let opening = Task { await controller.open(fixture.url) }
        try await waitForPrompt(controller, opening: opening)
        opening.cancel()
        await opening.value
        #expect(controller.recoveryPrompt == nil && !controller.busy)
        #expect(controller.snapshot == nil && controller.documentID == nil)
        #expect(try tip(fixture.url) == fixture.tip)
        let document = try Document(opening: fixture.url.path)
        #expect(try await document.snapshot().historyReport().recovery?.tip == fixture.tip)
        document.shutdown()
    }

    private actor PrepareCalls {
        var count = 0
        func receive(_ command: AudioCommand) -> AudioState {
            if case .prepare = command { count += 1 }
            return AudioState()
        }
    }

    private struct MissingWarning: Error {}

    private func waitForPrompt(_ controller: ProjectController, opening: Task<Void, Never>) async throws {
        let deadline = ContinuousClock.now + .seconds(5)
        while controller.recoveryPrompt == nil {
            guard ContinuousClock.now < deadline else {
                opening.cancel()
                Issue.record("recovery warning did not precede document publication")
                throw MissingWarning()
            }
            try await Task.sleep(for: .milliseconds(5))
        }
    }

    private func damagedProject() async throws -> (url: URL, tip: String, base: String) {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-recovery-\(UUID()).echode")
        do {
            let document = try Document(creating: url.path)
            _ = try await document.perform(#"{"type":"track.add","name":"A"}"#)
            document.shutdown()
        }
        let base = try tip(url)
        do {
            let document = try Document(opening: url.path)
            _ = try await document.perform(#"{"type":"track.add","name":"B"}"#)
            document.shutdown()
        }
        let latest = try tip(url)
        let object = url.appendingPathComponent(".git/objects/\(latest.prefix(2))/\(latest.dropFirst(2))")
        try FileManager.default.removeItem(at: object)
        return (url, latest, base)
    }

    private func tip(_ url: URL) throws -> String {
        try String(contentsOf: url.appendingPathComponent(".git/refs/echode/autosave/main"), encoding: .utf8)
            .trimmingCharacters(in: .whitespacesAndNewlines)
    }
}
