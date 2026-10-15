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
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
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
        #expect(controller.audio.playing)
        await controller.togglePlayback()
        #expect(controller.transportGeneration == 3)
        #expect(!controller.audio.playing)
        await controller.stopPlayback()
        #expect(controller.transportGeneration == 4)
        await controller.close()
        #expect(controller.transportGeneration == 0)
    }

}
