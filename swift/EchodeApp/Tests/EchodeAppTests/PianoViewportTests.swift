import AppKit
import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

@MainActor
struct PianoViewportTests {
    @Test func contractTrackSwitchesKeepTheSharedPianoViewport() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-viewport-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "Chords")
        let chords = try #require(controller.selection)
        await controller.insertNote(pitch: 48, beat: 0, length: 1)
        await controller.addTrack(name: "Melody")
        let melody = try #require(controller.selection)
        await controller.insertNote(pitch: 96, beat: 40, length: 1)
        let snapshot = try #require(controller.snapshot)
        let note = try #require(snapshot.pianoNeighbor(track: melody, current: nil, backwards: false))
        let navigation = CanvasNavigationState()
        navigation.prepare(document: controller.documentID, tracks: [chords, melody])
        navigation.pianoCamera = PianoCamera(originBeat: 4, topPitch: 90, pixelsPerBeat: 120, pixelsPerPitch: 28)
        let grid = PianoGridView(frame: NSRect(x: 0, y: 0, width: 650, height: 300))
        grid.setNavigation(navigation, documentID: controller.documentID)
        grid.setSnapshot(snapshot, track: chords, focus: nil, key: DisplayKey(), busy: false)
        let scroll = try #require(CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 2,
                                          wheel1: -100, wheel2: -500, wheel3: 0))
        grid.scrollWheel(with: NSEvent(cgEvent: scroll)!)
        let shared = grid.camera
        #expect(shared.originBeat > 0)
        grid.setSnapshot(snapshot, track: melody, focus: note.id, key: DisplayKey(), busy: false)
        #expect(grid.camera == shared, "Switching tracks must preserve the shared piano viewport")
        grid.setSnapshot(snapshot, track: chords, focus: nil, key: DisplayKey(), busy: false)
        #expect(grid.camera == shared)
        grid.stop()
        let recreated = PianoGridView(frame: grid.frame)
        recreated.setNavigation(navigation, documentID: controller.documentID)
        recreated.setSnapshot(snapshot, track: melody, focus: note.id, key: DisplayKey(), busy: false)
        #expect(recreated.camera == shared)
        // Even an empty document keeps its last view; stale views cannot write
        // into a new document that happens to reuse the same track IDs.
        navigation.prepare(document: controller.documentID, tracks: [])
        recreated.scrollWheel(with: NSEvent(cgEvent: scroll)!)
        #expect(navigation.pianoCamera == shared)
        let replacement = UUID()
        navigation.prepare(document: replacement, tracks: [chords, melody])
        grid.scrollWheel(with: NSEvent(cgEvent: scroll)!)
        #expect(navigation.pianoCamera == nil)
        recreated.setNavigation(navigation, documentID: replacement)
        recreated.setSnapshot(snapshot, track: melody, focus: nil, key: DisplayKey(), busy: false)
        #expect(recreated.camera == PianoCamera())
        #expect(navigation.pianoCamera == PianoCamera())
        #expect(recreated.navigate(backwards: false))
        #expect(recreated.camera != PianoCamera())
        #expect(controller.undoCount == 4)
        #expect(controller.playbackStart == 0)
        await controller.close()
    }
}
