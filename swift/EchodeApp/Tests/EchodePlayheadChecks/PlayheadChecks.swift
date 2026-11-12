import AppKit
import CoreGraphics
import EchodeBridge
import Foundation
import Testing

@testable import EchodeAppCore

/// The real-window scenarios run only under the AppKit probe event loop.
@MainActor
public struct PlayheadChecks {
    public init() {}
    public func stoppedLocateDrawsArrangeAndPianoAfterTheLinkPauses() async throws {
        let shown = try await ShownProject.make()
        try await shown.exercise(
            "locate",
            unverified: [
                "0→2 beats",
                "pause while stopped",
                "same feed does not redraw",
            ]
        ) {
            let ready = await shown.settled()
            try #require(ready, "\(dump(shown.arrange.timeline.measurement)) \(dump(shown.piano.grid.measurement))")
            try #require(shown.arrange.timeline.measurement.continuous == false)
            try #require(shown.piano.grid.measurement.continuous == false)
            try #require(shown.arrange.timeline.measurement.drawnTick == 0)
            try #require(shown.piano.grid.measurement.drawnTick == 0)

            let arrangeFrames = shown.arrange.timeline.measurement.frames
            let pianoFrames = shown.piano.grid.measurement.frames
            try await Task.sleep(for: .milliseconds(400))
            try #require(shown.arrange.timeline.measurement.frames == arrangeFrames)
            try #require(shown.piano.grid.measurement.frames == pianoFrames)
            try #require(shown.arrange.timeline.measurement.displayLinkPaused)

            await shown.controller.locate(beat: 2)
            shown.arrange.setPlayback(shown.feed)
            shown.piano.grid.setPlayback(shown.feed)
            try await Task.sleep(for: .milliseconds(200))
            try #require(shown.arrange.timeline.measurement.frames == arrangeFrames)
            try #require(shown.piano.grid.measurement.frames == pianoFrames)

            shown.noteTransport()
            let moved = await waitUntil {
                shown.arrange.timeline.measurement.drawnTick == 705_600 * 2
                    && shown.piano.grid.measurement.drawnTick == 705_600 * 2
                    && shown.arrange.timeline.measurement.displayLinkPaused
                    && shown.piano.grid.measurement.displayLinkPaused
            }
            try #require(
                moved,
                "stopped locate did not draw beat 2 \(dump(shown.arrange.timeline.measurement)) \(dump(shown.piano.grid.measurement))"
            )
            try #require(near(shown.arrange.timeline.measurement.playheadX, 161 + 2 * 72))
            try #require(near(shown.piano.grid.measurement.playheadX, 80 + 2 * 72))
        }
    }

    public func playDrawsTheLiveTickAndPauseFreezesIt() async throws {
        let shown = try await ShownProject.make(presented: false)
        do {
            await shown.controller.togglePlayback()
            let advanced = await waitUntil { shown.feed.position.playing && shown.feed.position.tick > 705_600 * 2 }
            try #require(advanced, "playback did not leave the start")
            shown.present()
            try await shown.requireVisible(unverified: [
                "playback continues",
                "pause",
            ])
            shown.noteTransport()
            let playing = await waitUntil {
                guard shown.arrange.timeline.measurement.drawnPlaying == true,
                    shown.piano.grid.measurement.drawnPlaying == true,
                    let arrangeTick = shown.arrange.timeline.measurement.drawnTick,
                    let pianoTick = shown.piano.grid.measurement.drawnTick
                else { return false }
                let heard = shown.feed.position.tick
                return arrangeTick > 705_600 * 2 && abs(arrangeTick - heard) < 705_600
                    && abs(pianoTick - heard) < 705_600
                    && !shown.arrange.timeline.measurement.displayLinkPaused
                    && !shown.piano.grid.measurement.displayLinkPaused
            }
            try #require(
                playing,
                "play did not draw the live playhead \(dump(shown.arrange.timeline.measurement)) \(dump(shown.piano.grid.measurement))"
            )
            let callbacks = shown.arrange.timeline.measurement.callbacks
            let pianoCallbacks = shown.piano.grid.measurement.callbacks
            try await Task.sleep(for: .milliseconds(200))
            try #require(shown.arrange.timeline.measurement.callbacks > callbacks + 2)
            try #require(shown.piano.grid.measurement.callbacks > pianoCallbacks + 2)

            // The play button pauses in place; Space would return to the start.
            await shown.controller.togglePause()
            let stopped = await waitUntil { !shown.feed.position.playing }
            try #require(stopped)
            shown.noteTransport()
            let paused = await waitUntil {
                shown.arrange.timeline.measurement.displayLinkPaused && shown.piano.grid.measurement.displayLinkPaused
                    && shown.arrange.timeline.measurement.drawnRevision == shown.arrange.timeline.measurement.revision
                    && shown.piano.grid.measurement.drawnRevision == shown.piano.grid.measurement.revision
                    && shown.arrange.timeline.measurement.drawnPlaying == false
                    && shown.piano.grid.measurement.drawnPlaying == false
                    && shown.arrange.timeline.measurement.drawnTick == shown.feed.position.tick
                    && shown.piano.grid.measurement.drawnTick == shown.feed.position.tick
            }
            try #require(
                paused,
                "pause did not freeze both playheads \(dump(shown.arrange.timeline.measurement)) \(dump(shown.piano.grid.measurement))"
            )
            let arrangeFrozen = shown.arrange.timeline.measurement.frames
            let pianoFrozen = shown.piano.grid.measurement.frames
            try await Task.sleep(for: .milliseconds(400))
            try #require(
                shown.arrange.timeline.measurement.frames == arrangeFrozen,
                "pause redraw arrange: frozen=\(arrangeFrozen) now=\(dump(shown.arrange.timeline.measurement))")
            try #require(
                shown.piano.grid.measurement.frames == pianoFrozen,
                "pause redraw piano: frozen=\(pianoFrozen) now=\(dump(shown.piano.grid.measurement))")
            try #require(shown.arrange.timeline.measurement.displayLinkPaused)
            try #require(shown.piano.grid.measurement.displayLinkPaused)
            try #require(!shown.feed.position.playing)
            try #require(!shown.controller.audio.playing)
            let facts = shown.displayFacts()
            await shown.close()
            PlayheadSpike.noteCompleted("play", facts: facts)
        } catch {
            let facts = shown.displayFacts()
            await shown.close()
            if !(error is PlayheadDisplayUnavailable) {
                PlayheadSpike.noteFailed(detail: String(describing: error), facts: facts)
            }
            throw error
        }
    }

    public func closedViewDoesNotDrawAndAReopenedOneDoes() async throws {
        let shown = try await ShownProject.make()
        try await shown.exercise(
            "closed",
            unverified: [
                "destroyed view",
                "reopened view draws the locate",
            ]
        ) {
            try #require(await shown.settled())
            shown.arrange.stop()
            await shown.controller.locate(beat: 3)
            shown.arrange.noteTransport(shown.controller.transportGeneration)
            try #require(!shown.arrange.timeline.rendererAvailable)

            let reopened = ArrangePaneView(frame: shown.arrange.frame)
            shown.content.addSubview(reopened)
            reopened.layoutSubtreeIfNeeded()
            let snapshot = try #require(shown.controller.snapshot)
            reopened.setSnapshot(snapshot, selected: shown.track)
            reopened.setPlayback(shown.feed)
            reopened.noteTransport(shown.controller.transportGeneration)
            let drawn = await waitUntil {
                reopened.timeline.measurement.drawnTick == 705_600 * 3
                    && reopened.timeline.measurement.displayLinkPaused
            }
            try #require(drawn, "reopened arrange missed the locate \(dump(reopened.timeline.measurement))")
            try #require(!shown.arrange.timeline.rendererAvailable)
            reopened.stop()
        }
    }

    public func planSwapAndDocumentSwitchKeepTheLiveView() async throws {
        let shown = try await ShownProject.make()
        try await shown.exercise(
            "document",
            unverified: [
                "plan swap",
                "document switch",
            ]
        ) {
            try #require(await shown.settled())
            try #require(
                await shown.controller.insertNote(
                    pitch: 64, beat: 1, length: 1, documentID: shown.controller.documentID!))
            let swapped = try #require(shown.controller.snapshot)
            shown.arrange.setSnapshot(swapped, selected: shown.track)
            shown.piano.grid.setSnapshot(swapped, track: shown.track, focus: nil, key: DisplayKey(), busy: false)
            await shown.controller.locate(beat: 1)
            shown.noteTransport()
            let moved = await waitUntil {
                shown.arrange.timeline.measurement.drawnTick == 705_600
                    && shown.piano.grid.measurement.drawnTick == 705_600
            }
            try #require(
                moved,
                "plan swap did not draw beat 1 \(dump(shown.arrange.timeline.measurement)) \(dump(shown.piano.grid.measurement))"
            )

            let stale = shown.arrange
            stale.stop()
            shown.piano.grid.stop()
            let other = FileManager.default.temporaryDirectory.appendingPathComponent(
                "echode-playhead-other-\(UUID()).echode")
            defer { try? FileManager.default.removeItem(at: other) }
            await shown.controller.open(other, creating: true)
            await shown.controller.addTrack(name: "next")
            let nextSnapshot = try #require(shown.controller.snapshot)
            let nextTrack = try #require(nextSnapshot.trackID(at: 0))
            let nextFeed = try #require(shown.controller.playback)
            let next = ArrangePaneView(frame: shown.arrange.frame)
            shown.content.addSubview(next)
            next.layoutSubtreeIfNeeded()
            next.setSnapshot(nextSnapshot, selected: nextTrack)
            next.setPlayback(nextFeed)
            await shown.controller.locate(beat: 2)
            next.noteTransport(shown.controller.transportGeneration)
            stale.noteTransport(shown.controller.transportGeneration)
            let nextDrawn = await waitUntil {
                next.timeline.measurement.drawnTick == 705_600 * 2 && next.timeline.measurement.displayLinkPaused
            }
            try #require(nextDrawn, "the new document did not draw its own locate \(dump(next.timeline.measurement))")
            try #require(!stale.timeline.rendererAvailable)
            next.stop()
        }
    }
}

@MainActor
private enum PlayheadSpike {
    private static let required = ["locate", "play", "closed", "document"]
    private static var status = "running"
    private static var facts: PlayheadDisplayFacts?
    private static var unverified: [String] = []
    private static var completed: Set<String> = []
    private static var detail: String?

    static func recordInsufficient(facts: PlayheadDisplayFacts, unverified items: [String]) {
        status = "insufficient-environment"
        self.facts = facts
        for item in items where !unverified.contains(item) {
            unverified.append(item)
        }
        write()
        let list = unverified.joined(separator: "; ")
        fputs(
            "spike-playhead: window is not on the current Space without occlusion (\(facts)). Unverified: \(list)\n",
            stderr)
    }

    static func noteCompleted(_ name: String, facts: PlayheadDisplayFacts) {
        guard status == "running" else { return }
        self.facts = facts
        completed.insert(name)
        if required.allSatisfy({ completed.contains($0) }) {
            status = "passed"
            unverified = []
            detail = nil
            write()
            fputs("spike-playhead: window stayed on the active Space without occlusion; expectations ran\n", stderr)
        }
    }

    static func noteFailed(detail: String, facts: PlayheadDisplayFacts) {
        guard status == "running" else { return }
        status = "failed"
        self.facts = facts
        self.detail = detail
        write()
    }

    private static func reportURL() -> URL {
        if let path = ProcessInfo.processInfo.environment["ECHODE_SPIKE_PLAYHEAD_REPORT"], !path.isEmpty {
            return URL(fileURLWithPath: path)
        }
        return URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
            .appendingPathComponent("target/spike-playhead.json")
    }

    private static func write() {
        let facts = facts
        let report = SpikePlayheadReport(
            status: status,
            screens: facts?.screens ?? 0,
            asleep: facts?.asleep ?? false,
            visible: facts?.visible ?? false,
            onActiveSpace: facts?.onActiveSpace ?? false,
            occluded: facts?.occluded ?? true,
            miniaturized: facts?.miniaturized ?? false,
            unverified: unverified,
            detail: detail
        )
        let url = reportURL()
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        guard let data = try? encoder.encode(report) else { return }
        try? FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try? data.write(to: url)
    }
}

private struct PlayheadDisplayUnavailable: Error, CustomStringConvertible {
    var facts: PlayheadDisplayFacts
    var unverified: [String]

    var description: String {
        "window is not on the current Space without occlusion (\(facts)). Unverified: \(unverified.joined(separator: "; "))"
    }
}

private struct PlayheadDisplayFacts: Codable, CustomStringConvertible {
    var screens: Int
    var asleep: Bool
    var visible: Bool
    var onActiveSpace: Bool
    var occluded: Bool
    var miniaturized: Bool

    var description: String {
        "screens=\(screens) asleep=\(asleep) visible=\(visible) onActiveSpace=\(onActiveSpace) occluded=\(occluded) miniaturized=\(miniaturized)"
    }
}

private struct SpikePlayheadReport: Codable {
    var status: String
    var screens: Int
    var asleep: Bool
    var visible: Bool
    var onActiveSpace: Bool
    var occluded: Bool
    var miniaturized: Bool
    var unverified: [String]
    var detail: String?
}

@MainActor
private final class ShownProject {
    let url: URL
    let controller: ProjectController
    let window: NSWindow
    let content: NSView
    let arrange: ArrangePaneView
    let piano: PianoRollView
    let feed: PlaybackFeed
    let track: UUID
    private var didClose = false

    static func make(presented: Bool = true) async throws -> ShownProject {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-playhead-\(UUID()).echode")
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "pad")
        await controller.insertNote(pitch: 60, beat: 0, length: 8, documentID: controller.documentID!)
        let snapshot = try #require(controller.snapshot)
        let track = try #require(snapshot.trackID(at: 0))
        let feed = try #require(controller.playback)
        let app = NSApplication.shared
        app.setActivationPolicy(.regular)
        let window = NSWindow(
            contentRect: NSRect(x: 80, y: 80, width: 900, height: 650),
            styleMask: [.titled, .closable], backing: .buffered, defer: false)
        window.title = "Echode playhead"
        window.isReleasedWhenClosed = false
        let content = NSView(frame: NSRect(x: 0, y: 0, width: 900, height: 650))
        let arrange = ArrangePaneView(frame: NSRect(x: 0, y: 330, width: 900, height: 320))
        let piano = PianoRollView(frame: NSRect(x: 0, y: 0, width: 900, height: 330))
        arrange.setSnapshot(snapshot, selected: track)
        arrange.setPlayback(feed)
        piano.grid.setSnapshot(snapshot, track: track, focus: nil, key: DisplayKey(), busy: false)
        piano.grid.setPlayback(feed)
        arrange.layoutSubtreeIfNeeded()
        piano.layoutSubtreeIfNeeded()
        let shown = ShownProject(
            url: url, controller: controller, window: window, content: content,
            arrange: arrange, piano: piano, feed: feed, track: track)
        if presented { shown.present() }
        return shown
    }

    private init(
        url: URL, controller: ProjectController, window: NSWindow, content: NSView,
        arrange: ArrangePaneView, piano: PianoRollView, feed: PlaybackFeed, track: UUID
    ) {
        self.url = url
        self.controller = controller
        self.window = window
        self.content = content
        self.arrange = arrange
        self.piano = piano
        self.feed = feed
        self.track = track
    }

    func present() {
        content.addSubview(arrange)
        content.addSubview(piano)
        window.contentView = content
        // Joining every Space is only an attempt to reach the current one.
        // Expectations still wait until that is actually true and the window is not occluded.
        window.collectionBehavior = .canJoinAllSpaces
        window.makeKeyAndOrderFront(nil)
        window.orderFrontRegardless()
        NSRunningApplication.current.activate(options: [.activateAllWindows])
    }

    /// Major drawing expectations run only after this returns. A timeout records the
    /// display and the items that were not checked, then fails the test.
    func requireVisible(unverified: [String]) async throws {
        let ready = await waitUntil(timeout: .seconds(5)) {
            window.isVisible && window.isOnActiveSpace && window.occlusionState.contains(.visible)
                && !window.isMiniaturized
        }
        let facts = displayFacts()
        guard ready else {
            PlayheadSpike.recordInsufficient(facts: facts, unverified: unverified)
            throw PlayheadDisplayUnavailable(facts: facts, unverified: unverified)
        }
        fputs("spike-playhead: window is on the active Space and not occluded (\(facts))\n", stderr)
    }

    func displayFacts() -> PlayheadDisplayFacts {
        PlayheadDisplayFacts(
            screens: NSScreen.screens.count,
            asleep: CGDisplayIsAsleep(CGMainDisplayID()) != 0,
            visible: window.isVisible,
            onActiveSpace: window.isOnActiveSpace,
            occluded: !window.occlusionState.contains(.visible),
            miniaturized: window.isMiniaturized
        )
    }

    func exercise(_ name: String, unverified: [String], _ body: @MainActor () async throws -> Void) async throws {
        do {
            try await requireVisible(unverified: unverified)
            try await body()
            let facts = displayFacts()
            await close()
            PlayheadSpike.noteCompleted(name, facts: facts)
        } catch {
            let facts = displayFacts()
            await close()
            if !(error is PlayheadDisplayUnavailable) {
                PlayheadSpike.noteFailed(detail: String(describing: error), facts: facts)
            }
            throw error
        }
    }

    func settled() async -> Bool {
        await waitUntil {
            arrange.timeline.measurement.displayLinkPaused && arrange.timeline.measurement.frames >= 1
                && piano.grid.measurement.displayLinkPaused && piano.grid.measurement.frames >= 1
                && arrange.timeline.measurement.drawnRevision == arrange.timeline.measurement.revision
                && piano.grid.measurement.drawnRevision == piano.grid.measurement.revision
        }
    }

    func noteTransport() {
        arrange.noteTransport(controller.transportGeneration)
        piano.grid.noteTransport(controller.transportGeneration)
    }

    func close() async {
        guard !didClose else { return }
        didClose = true
        arrange.stop()
        piano.grid.stop()
        window.close()
        await controller.close()
        try? FileManager.default.removeItem(at: url)
    }
}

private func dump(_ measurement: ArrangeMeasurement) -> String {
    "frames=\(measurement.frames) paused=\(measurement.displayLinkPaused) tick=\(String(describing: measurement.drawnTick)) playing=\(String(describing: measurement.drawnPlaying)) x=\(String(describing: measurement.playheadX)) callbacks=\(measurement.callbacks) wakes=\(measurement.wakeCount) resumes=\(measurement.resumeCount)"
}

private func near(_ value: Double?, _ expected: Double) -> Bool {
    guard let value else { return false }
    return abs(value - expected) < 1
}

@MainActor
private func waitUntil(timeout: Duration = .seconds(3), _ predicate: @MainActor () -> Bool) async -> Bool {
    let deadline = ContinuousClock.now.advanced(by: timeout)
    while ContinuousClock.now < deadline {
        if predicate() { return true }
        try? await Task.sleep(for: .milliseconds(20))
    }
    return predicate()
}
