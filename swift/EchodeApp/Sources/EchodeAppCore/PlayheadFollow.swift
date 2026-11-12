import AppKit
import EchodeBridge
import QuartzCore

/// Paging rule for an editor's time origin (ADR 0159). A playhead that leaves
/// the view moves it by a whole page instead of scrolling every frame.
enum PlayheadFollow {
    /// The origin in beats that shows `tick`, or nil while it is already shown.
    /// A playhead past either edge lands on the left edge.
    static func origin(showing tick: Int64, from origin: Double, span: Double) -> Double? {
        guard origin.isFinite, span.isFinite, span > 0 else { return nil }
        let beat = Double(tick) / Tick.perBeat
        if beat >= origin, beat < origin + span { return nil }
        return max(0, beat)
    }

    /// A manual time scroll leads with its horizontal part. Vertical travel
    /// through tracks or pitches keeps following.
    static func isTimeScroll(x: Double, y: Double) -> Bool { abs(x) > abs(y) }

    /// The main-thread link sleeps unless it follows a playing feed or a
    /// transport change is still reaching the audio clock.
    static func linkPaused(following: Bool, playing: Bool, now: Double, settleUntil: Double) -> Bool {
        !following || (!playing && now >= settleUntil)
    }
}

/// Reads the live feed on the main thread for one editor. The renderer keeps
/// its own link; this one runs only while following a playing feed, or for a
/// moment after a transport change or after following is turned on.
@MainActor
final class PlayheadFollower: NSObject {
    /// A stop or locate reaches the published position after its command
    /// returns. The link keeps looking this long so the new position is shown.
    static let settle = 0.3
    private(set) var following = false
    /// The last pause decision, kept without a window so it can be checked.
    private(set) var awake = false
    var feed: PlaybackFeed? { didSet { if feed != oldValue { reconsider() } } }
    /// Shows a playhead tick. The editor skips it while a press holds its view.
    var show: ((Int64) -> Void)?
    var now: () -> Double = CACurrentMediaTime
    private var settleUntil = 0.0
    private var link: CADisplayLink?

    func setFollowing(_ value: Bool) {
        guard following != value else { return }
        following = value
        // Turning following on shows where the playhead is, even when stopped.
        if value { settleUntil = now() + Self.settle }
        reconsider()
    }

    /// Play, pause, stop, or a locate.
    func noteTransport() {
        settleUntil = now() + Self.settle
        reconsider()
    }

    func attach(to view: NSView) {
        guard link == nil else { return }
        let link = view.displayLink(target: self, selector: #selector(frame(_:)))
        link.isPaused = true
        link.add(to: .main, forMode: .common)
        self.link = link
        reconsider()
    }

    func detach() {
        link?.invalidate()
        link = nil
        awake = false
    }

    /// One look at the live feed. The display link calls this each frame.
    func step() {
        guard let feed else {
            setAwake(false)
            return
        }
        let position = feed.position
        step(tick: position.tick, playing: position.playing)
    }

    func step(tick: Int64, playing: Bool) {
        guard following else {
            setAwake(false)
            return
        }
        show?(tick)
        setAwake(!PlayheadFollow.linkPaused(following: true, playing: playing, now: now(), settleUntil: settleUntil))
    }

    @objc private func frame(_ link: CADisplayLink) { step() }

    /// Wakes the link; its next frame decides whether to sleep again.
    private func reconsider() { setAwake(following && feed != nil) }

    private func setAwake(_ value: Bool) {
        awake = value
        // Writing the same value can drop the next vsync.
        if let link, link.isPaused == value { link.isPaused = !value }
    }
}
