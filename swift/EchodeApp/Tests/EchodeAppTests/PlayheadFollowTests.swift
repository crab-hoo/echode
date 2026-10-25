import AppKit
import EchodeBridge
import Foundation
import SwiftUI
import Testing
@testable import EchodeAppCore

@MainActor
struct PlayheadFollowTests {
    private let beat: Int64 = 705_600

    @Test func pagingLeavesAShownPlayheadAndPutsAnyOtherOnTheLeftEdge() {
        #expect(PlayheadFollow.origin(showing: 0, from: 0, span: 10) == nil)
        #expect(PlayheadFollow.origin(showing: beat * 999 / 100, from: 0, span: 10) == nil)
        // The right edge is outside the half-open view, as when drawing.
        #expect(PlayheadFollow.origin(showing: beat * 10, from: 0, span: 10) == 10)
        #expect(PlayheadFollow.origin(showing: beat * 25, from: 0, span: 10) == 25)
        // A loop or a stop that returns before the view shows that position.
        #expect(PlayheadFollow.origin(showing: beat * 2, from: 8, span: 10) == 2)
        #expect(PlayheadFollow.origin(showing: beat * 2, from: 0, span: 0) == nil)
        #expect(PlayheadFollow.origin(showing: beat * 2, from: .nan, span: 10) == nil)
        #expect(PlayheadFollow.origin(showing: beat * 2, from: 0, span: .infinity) == nil)

        #expect(PlayheadFollow.isTimeScroll(x: -5, y: 1))
        #expect(!PlayheadFollow.isTimeScroll(x: 0.5, y: -8))
        #expect(!PlayheadFollow.isTimeScroll(x: 3, y: 3) && !PlayheadFollow.isTimeScroll(x: 0, y: 0))

        #expect(PlayheadFollow.linkPaused(following: false, playing: true, now: 0, settleUntil: 1))
        #expect(!PlayheadFollow.linkPaused(following: true, playing: true, now: 2, settleUntil: 1))
        #expect(!PlayheadFollow.linkPaused(following: true, playing: false, now: 0.5, settleUntil: 1))
        #expect(PlayheadFollow.linkPaused(following: true, playing: false, now: 1, settleUntil: 1))
    }

    /// The main-thread link wakes on a transport change or when following is
    /// turned on, keeps running while playing, and sleeps once a stop settles.
    @Test func followerSleepsOnceAStoppedPositionSettles() {
        let follower = PlayheadFollower()
        var clock = 0.0
        follower.now = { clock }
        var shown: [Int64] = []
        follower.show = { shown.append($0) }
        follower.step(tick: 1, playing: true)
        #expect(shown.isEmpty && !follower.awake, "nothing follows until it is turned on")

        follower.setFollowing(true)
        clock = 0.1; follower.step(tick: 2, playing: false)
        #expect(shown == [2] && follower.awake)
        clock = 0.4; follower.step(tick: 2, playing: false)
        #expect(!follower.awake)
        follower.noteTransport()
        clock = 0.6; follower.step(tick: 3, playing: false)
        #expect(follower.awake)
        clock = 9; follower.step(tick: 4, playing: true)
        #expect(follower.awake && shown == [2, 2, 3, 4])

        follower.setFollowing(false)
        follower.step(tick: 5, playing: true)
        #expect(!follower.awake && shown.last == 4)
        follower.noteTransport()
        #expect(!follower.awake, "a transport change does not wake a follower that is off")
    }

    /// Both editors page to the product playhead, keep a shown playhead, wait
    /// for presses, keep following through vertical scrolls and zoom, and stop
    /// after a time scroll until following is turned on again (ADR 0159).
    @Test func contractBothEditorsPageToThePlayheadUntilATimeScroll() async throws {
        let controller = ProjectController()
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-follow-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        let snapshot = try #require(controller.snapshot), track = try #require(controller.selection)
        let feed = try #require(controller.playback)
        let undo = controller.undoCount

        let pane = ArrangePaneView(frame: NSRect(x: 0, y: 0, width: 900, height: 200))
        pane.layout(); pane.setSnapshot(snapshot, selected: track)
        pane.timeline.setRuler(start: controller.playbackStart, loop: nil, busy: false)
        let piano = PianoGridView(frame: NSRect(x: 0, y: 0, width: 640, height: 400))
        piano.setSnapshot(snapshot, track: track, focus: nil, key: DisplayKey(), busy: false)
        func presentPiano() {
            let input = CanvasInput(snapshot: snapshot, selected: track, camera: ArrangeCamera(), width: 640, height: 400, scale: 1)
            piano.present(PianoFrame(input: input, piano: PianoRenderInput(track: track, focus: nil, camera: piano.camera,
                                                                           key: DisplayKey(), preview: nil)))
        }
        var released = 0
        pane.onManualScroll = { released += 1 }
        piano.onManualScroll = { released += 1 }
        pane.setPlayback(feed); piano.setPlayback(feed)
        pane.setFollowing(true); piano.follower.setFollowing(true)
        func step() { pane.follower.step(); piano.follower.step() }
        func move(to position: Int64) async {
            await controller.locate(beat: Double(position))
            #expect(feed.position.tick == position * beat)
            step()
        }

        // The arrange shows (900 - 161) / 72 beats and the piano roll 560 / 72.
        await controller.setPlaybackStart(beat: 20)
        #expect(feed.position.tick == 20 * beat)
        step()
        #expect(pane.viewport.originBeat == 20 && piano.camera.originBeat == 20)
        let frame = ArrangeFrame(input: pane.timeline.renderInput)
        #expect(frame.playheadX == frame.timelineOrigin, "the paged playhead is drawn at the left edge")
        await move(to: 24)
        #expect(pane.viewport.originBeat == 20 && piano.camera.originBeat == 20)
        await move(to: 29)
        #expect(pane.viewport.originBeat == 20 && piano.camera.originBeat == 29)
        await move(to: 3)
        #expect(pane.viewport.originBeat == 3 && piano.camera.originBeat == 3)

        // Travel through tracks or pitches and zoom keep following.
        pane.timeline.onScroll?(2, -40)
        pane.timeline.onZoom?(2, [], .zero)
        let vertical = try #require(CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 2, wheel1: -40, wheel2: 2, wheel3: 0))
        piano.scrollWheel(with: try #require(NSEvent(cgEvent: vertical)))
        #expect(released == 0 && pane.follower.following && piano.follower.following)

        // A ruler press holds each view; the first frame after release pages.
        let arrangeOrigin = pane.viewport.originBeat, pianoOrigin = piano.camera.originBeat
        let rulerX = pane.timeline.renderInput.timelineOrigin + 10
        func mouse(_ type: NSEvent.EventType, _ point: CGPoint, in view: NSView) throws -> NSEvent {
            try #require(NSEvent.mouseEvent(with: type, location: view.convert(point, to: nil), modifierFlags: [], timestamp: 0,
                windowNumber: 0, context: nil, eventNumber: 1, clickCount: 1, pressure: 1))
        }
        pane.timeline.mouseDown(with: try mouse(.leftMouseDown, CGPoint(x: rulerX, y: 10), in: pane.timeline))
        presentPiano()
        let pianoRuler = CGPoint(x: piano.pianoLayout.gridOrigin.x + 10, y: 10)
        piano.mouseDown(with: try mouse(.leftMouseDown, pianoRuler, in: piano))
        #expect(pane.timeline.holdsViewport)
        await move(to: 50)
        #expect(pane.viewport.originBeat == arrangeOrigin && piano.camera.originBeat == pianoOrigin)
        pane.timeline.mouseUp(with: try mouse(.leftMouseUp, CGPoint(x: rulerX, y: 10), in: pane.timeline))
        piano.mouseUp(with: try mouse(.leftMouseUp, pianoRuler, in: piano))
        step()
        #expect(pane.viewport.originBeat == 50 && piano.camera.originBeat == 50)

        // A time scroll in either editor turns its following off and reports it.
        pane.timeline.onScroll?(-72, 4)
        #expect(released == 1 && !pane.follower.following && piano.follower.following)
        let scrolled = pane.viewport.originBeat
        await move(to: 80)
        #expect(pane.viewport.originBeat == scrolled && piano.camera.originBeat == 80)
        let horizontal = try #require(CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 2, wheel1: 3, wheel2: -40, wheel3: 0))
        piano.scrollWheel(with: try #require(NSEvent(cgEvent: horizontal)))
        #expect(released == 2 && !piano.follower.following)
        let pianoScrolled = piano.camera.originBeat
        await move(to: 120)
        #expect(pane.viewport.originBeat == scrolled && piano.camera.originBeat == pianoScrolled)

        // Turning it on again shows the stopped playhead at once.
        pane.setFollowing(true); piano.follower.setFollowing(true)
        #expect(pane.follower.awake && piano.follower.awake)
        step()
        #expect(pane.viewport.originBeat == 120 && piano.camera.originBeat == 120)
        #expect(controller.undoCount == undo && controller.playbackStart == 20 * beat)
        pane.stop(); piano.stop()
        #expect(!pane.follower.awake && !piano.follower.awake)
        await controller.close()
    }

    /// Following is a per-document display state in the workspace, toggled by
    /// the Playback menu, the palette and the transport, outside Undo.
    @Test func contractFollowingIsADisplayStateThatEachDocumentStartsWith() async throws {
        let controller = ProjectController()
        let state = WorkspaceState(defaults: .init())
        let session = WorkspaceSession(controller: controller, state: state)
        #expect(state.followsPlayhead && !session.enabled(.followPlayhead))
        let first = FileManager.default.temporaryDirectory.appendingPathComponent("echode-follow-a-\(UUID()).echode")
        let second = FileManager.default.temporaryDirectory.appendingPathComponent("echode-follow-b-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: first); try? FileManager.default.removeItem(at: second) }
        await controller.open(first, creating: true)
        state.synchronize(controller)
        let undo = controller.undoCount
        #expect(session.enabled(.followPlayhead) && session.isSelected(.followPlayhead))
        session.perform(.followPlayhead)
        #expect(!state.followsPlayhead && !session.isSelected(.followPlayhead))
        state.synchronize(controller)
        #expect(!state.followsPlayhead, "the same document keeps following off")
        #expect(controller.undoCount == undo)
        session.perform(.followPlayhead)
        #expect(state.followsPlayhead)
        session.perform(.followPlayhead)
        await controller.open(second, creating: true)
        state.synchronize(controller)
        #expect(state.followsPlayhead, "another document starts following")
        let command = WorkspaceCommand.followPlayhead
        #expect(command.isToggle && !command.isLayoutToggle && command.shortcut == nil)
        await controller.close()
    }

    /// The SwiftUI canvases pass the workspace flag down and report a time scroll back up.
    @Test func canvasesCarryTheWorkspaceFlagBothWays() async throws {
        let controller = ProjectController()
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-follow-host-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        let state = WorkspaceState(defaults: .init())
        state.synchronize(controller)
        let host = NSHostingView(rootView: FollowHost(controller: controller, state: state))
        host.frame = NSRect(x: 0, y: 0, width: 900, height: 600)
        host.layoutSubtreeIfNeeded()
        let pane = try #require(Self.find(ArrangePaneView.self, in: host))
        let piano = try #require(Self.find(PianoGridView.self, in: host))
        #expect(pane.follower.following && piano.follower.following)
        pane.timeline.onScroll?(-30, 0)
        #expect(!state.followsPlayhead)
        host.layoutSubtreeIfNeeded()
        #expect(!piano.follower.following, "the other editor stops following with the workspace flag")
        state.followsPlayhead = true
        host.layoutSubtreeIfNeeded()
        #expect(pane.follower.following && piano.follower.following)
        await controller.close()
    }

    private static func find<V: NSView>(_ type: V.Type, in view: NSView) -> V? {
        if let match = view as? V { return match }
        for child in view.subviews { if let match = find(type, in: child) { return match } }
        return nil
    }
}

private struct FollowHost: View {
    let controller: ProjectController
    @Bindable var state: WorkspaceState
    var body: some View {
        if let snapshot = controller.snapshot, let track = controller.selection {
            VStack {
                ArrangeCanvas(snapshot: snapshot, selection: .constant(track), playback: controller.playback,
                              followsPlayhead: state.followsPlayhead, onManualScroll: { state.followsPlayhead = false })
                PianoCanvas(snapshot: snapshot, track: track, focus: .constant(nil), key: DisplayKey(), busy: false,
                            insert: { _, _, _ in }, edit: { _, _ in }, delete: { _ in }, playback: controller.playback,
                            followsPlayhead: state.followsPlayhead, onManualScroll: { state.followsPlayhead = false })
            }
        }
    }
}
