import AppKit
import CoreGraphics
import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

@MainActor
@Suite(.serialized)
struct PlayheadTests {
    @Test func transportCommandsRequestAFrameAndPollingDoesNot() async {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-playhead-gen-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController { document, command in
            guard case .play = command else { return try await document.audio(command) }
            // This test checks UI invalidation, not HAL. Keep the real bridge
            // for position and other commands, but never open an output device.
            let state = try await document.audio(.status)
            return try JSONDecoder().decode(AudioState.self, from: Data("""
                {"playing":true,"running":false,"opening":false,"tick":\(state.tick),"devices":[],"metronome":\(state.metronome)}
                """.utf8))
        }
        await controller.open(url, creating: true)
        await controller.addTrack(name: "pad")
        #expect(controller.transportGeneration == 0)
        await controller.toggleMetronome()
        await controller.toggleLoop()
        #expect(controller.transportGeneration == 0)
        await controller.locate(beat: 2)
        #expect(controller.transportGeneration == 1)
        #expect(controller.audio.tick == 705_600 * 2)
        await controller.togglePlayback()
        #expect(controller.transportGeneration == 2)
        #expect(controller.audio.playing && !controller.audio.running)
        await controller.togglePlayback()
        #expect(controller.transportGeneration == 3)
        #expect(!controller.audio.playing)
        await controller.stopPlayback()
        #expect(controller.transportGeneration == 4)
        await controller.close()
        #expect(controller.transportGeneration == 0)
    }

}
