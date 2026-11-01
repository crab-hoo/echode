import AppKit
import EchodeBridge
import Testing
@testable import EchodeAppCore

@MainActor
struct MixerStepTests {
    @Test func contractAccessibleStepsPreserveCapturedGroupOffsetsAndHistory() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-mixer-step-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        var tracks: [UUID] = []
        for name in ["anchor", "quieter", "outside"] {
            await controller.addTrack(name: name)
            tracks.append(try #require(controller.selection))
        }
        #expect(await controller.setMixer(tracks[0], field: .gain, value: 0.5))
        #expect(await controller.setMixer(tracks[1], field: .gain, value: 0.25))
        #expect(await controller.setMixer(tracks[0], field: .pan, value: -0.5))
        #expect(await controller.setMixer(tracks[1], field: .pan, value: 0.25))
        controller.selection = tracks[0]
        controller.selectTrack(tracks[1], change: .toggle)
        let undo = controller.undoCount
        let window = NSWindow(contentRect: CGRect(x: 0, y: 0, width: 220, height: 192), styleMask: [.borderless], backing: .buffered, defer: true)
        let header = ArrangeHeaderView(frame: CGRect(x: 0, y: 0, width: 220, height: 192))
        window.contentView = header
        header.documentID = controller.documentID
        header.mixActions = controller.arrangeMixActions(document: controller.documentID)
        header.setSnapshot(try #require(controller.snapshot), selected: controller.selection, selectedTracks: controller.selectedTracks)
        func control(_ field: MixerField) throws -> NSAccessibilityElement {
            let row = try #require((header.accessibilityChildren() as? [NSAccessibilityElement])?.first)
            return try #require((row.accessibilityChildren() as? [NSAccessibilityElement])?.first {
                $0.accessibilityIdentifier() == "\(tracks[0])/\(field.rawValue)"
            })
        }
        let gain = try control(.gain)
        #expect(gain.accessibilityPerformIncrement())
        #expect(gain.accessibilityPerformIncrement())
        // No tree refresh between inputs: each step must use the edit turn's current values.
        let deadline = ContinuousClock.now.advanced(by: .seconds(5))
        while controller.undoCount < undo + 2 && ContinuousClock.now < deadline { await Task.yield() }
        #expect(controller.undoCount == undo + 2)
        let factor = Float(pow(10.0, 2.0 / 20))
        #expect(abs(try #require(controller.mixer(tracks[0])).gain.value - 0.5 * factor) < 1e-6)
        #expect(abs(try #require(controller.mixer(tracks[1])).gain.value - 0.25 * factor) < 1e-6,
            "accessible volume steps must preserve the captured group's dB offsets")
        let pan = try control(.pan)
        #expect(pan.accessibilityPerformDecrement())
        // Selection changes before the edit task runs, not before event capture.
        controller.selection = tracks[2]
        let panDeadline = ContinuousClock.now.advanced(by: .seconds(5))
        while controller.undoCount < undo + 3 && ContinuousClock.now < panDeadline { await Task.yield() }
        #expect(abs(try #require(controller.mixer(tracks[0])).pan.value + 0.55) < 1e-6)
        #expect(abs(try #require(controller.mixer(tracks[1])).pan.value - 0.20) < 1e-6,
            "accessible pan steps must retain the event's selected tracks")
        #expect(controller.mixer(tracks[2])?.gain.value == 1 && controller.mixer(tracks[2])?.pan.value == 0)
        #expect(controller.undoCount == undo + 3 && controller.selection == tracks[2])
        await controller.undo()
        #expect(controller.mixer(tracks[0])?.pan.value == -0.5 && controller.mixer(tracks[1])?.pan.value == 0.25)
        await controller.redo()
        let recorded = tracks.map { controller.mixer($0) }
        await controller.close(); await controller.open(url)
        #expect(tracks.map { controller.mixer($0) } == recorded)
        await controller.close()
    }
}
