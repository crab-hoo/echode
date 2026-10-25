import AppKit
import EchodeBridge

/// One clip drag, captured at release before any document work starts (ADR 0154).
public struct ClipMove: Sendable, Equatable {
    let document: UUID
    let clip: UUID
    let track: UUID
    let start: Int64
    /// Command at release: a copy with its own content instead of a move.
    let copy: Bool
}

/// Where the dragged clip would land, drawn over the arrange while dragging.
struct ClipMovePreview: Sendable, Equatable {
    let row: Int
    let track: UUID
    let start: Int64
    let length: Int64
    let copy: Bool
}

/// A press on a clip body. It becomes a drag after `ClipMoveTarget.threshold` points.
struct ClipMovePress {
    let clip: ClipMix
    let point: CGPoint
    /// Timeline beat under the press, so a scroll or zoom mid-drag keeps the travel.
    let beat: Double
    var preview: ClipMovePreview?
}

enum ClipMoveTarget {
    /// Pointer travel before a press on a clip moves it. A click only selects.
    static let threshold = 3.0

    /// New start after `beats` of travel from `start`. The arrange grid draws
    /// quarter-note lines and the start snaps to the nearest one; Shift keeps
    /// the pointer's tick.
    static func start(from start: Int64, beats: Double, free: Bool) -> Int64? {
        guard beats.isFinite else { return nil }
        let beat = Double(start) / 705_600 + beats
        return PianoCamera.ticks(free ? beat : beat.rounded()).map { max(0, $0) }
    }
}

extension ArrangeRows {
    /// The row at `y`, clamped to the first and last track so a drag past
    /// either edge keeps a target.
    func clampedIndex(at y: Double) -> Int? {
        guard count > 0, y.isFinite, rowHeight > 0 else { return nil }
        let value = (origin + y / rowHeight).rounded(.down)
        return Int(min(Double(count - 1), max(0, value)))
    }
}

extension ArrangeView {
    func pressClip(_ clip: ClipMix, at point: CGPoint) {
        movePress = ClipMovePress(clip: clip, point: point, beat: timelineBeat(at: point))
    }

    func dragClip(to point: CGPoint, flags: NSEvent.ModifierFlags) {
        guard var press = movePress, let snapshot else { return }
        if press.preview == nil, hypot(point.x - press.point.x, point.y - press.point.y) < ClipMoveTarget.threshold { return }
        let rows = camera.rows(trackCount: snapshot.trackCount, height: renderInput.arrangeHeight)
        guard let row = rows.clampedIndex(at: Double(point.y) - renderInput.arrangeTop), let track = snapshot.trackID(at: row),
              let start = ClipMoveTarget.start(from: press.clip.start, beats: timelineBeat(at: point) - press.beat,
                                               free: flags.contains(.shift)) else { return }
        press.preview = ClipMovePreview(row: row, track: track, start: start, length: press.clip.length, copy: flags.contains(.command))
        movePress = press
        refresh()
    }

    /// Release sends one request when the clip lands somewhere new. Escape
    /// and a cancelled gesture drop the preview without an edit.
    func finishClipMove(at point: CGPoint?, flags: NSEvent.ModifierFlags) {
        if let point { dragClip(to: point, flags: flags) }
        guard let press = movePress else { return }
        movePress = nil
        refresh()
        guard point != nil, let preview = press.preview, let document = audioDocument,
              preview.track != press.clip.track || preview.start != press.clip.start else { return }
        onClipMove?(ClipMove(document: document, clip: press.clip.id, track: preview.track,
                             start: preview.start, copy: flags.contains(.command)))
    }

    private func timelineBeat(at point: CGPoint) -> Double {
        let origin = renderInput.header.map { $0.width + 1 } ?? 0
        return camera.originBeat + (Double(point.x) - origin) / camera.pixelsPerBeat
    }
}

extension ArrangeFrame {
    /// The dragged clip where it would land. A copy and a move use different colors.
    var clipMoveRect: CGRect? {
        guard let move = input.clipMove, let snapshot = input.snapshot else { return nil }
        let row = camera.rows(trackCount: snapshot.trackCount, height: input.arrangeHeight).rect(at: move.row, width: 0)
        return CGRect(x: timelineOrigin + (Double(move.start) / 705_600 - camera.originBeat) * camera.pixelsPerBeat,
                      y: input.arrangeTop + row.minY, width: Double(move.length) * camera.pixelsPerBeat / 705_600, height: max(1, row.height - 1))
    }

    var clipMoveInstances: [Instance] {
        guard let rect = clipMoveRect, let move = input.clipMove else { return [] }
        let color = move.copy ? SIMD4<Float>(0.55, 0.86, 0.48, 0.6) : SIMD4<Float>(0.95, 0.78, 0.38, 0.6)
        return [Instance(rect: SIMD4(Float(rect.minX), Float(rect.minY), Float(rect.width), Float(rect.height)) * Float(input.scale), color: color)]
    }
}

extension WorkspaceSession {
    @discardableResult
    func moveClip(_ move: ClipMove) -> Task<Bool, Never>? {
        guard !controller.busy, controller.documentID == move.document else { return nil }
        return Task { await controller.moveClip(move) }
    }
}

extension ProjectController {
    /// One undo step. The landed clip keeps the focus on its new track.
    @discardableResult
    func moveClip(_ move: ClipMove) async -> Bool {
        let applied = await edit({ [self] in
            guard documentID == move.document, move.start >= 0, let clip = snapshot?.clipMix(move.clip),
                  tracks.contains(where: { $0.id == move.track }),
                  clip.track != move.track || clip.start != move.start else { return nil }
            return ["type": move.copy ? "clip.duplicate" : "clip.move", "clip": move.clip.uuidString,
                    "track": move.track.uuidString, "start": move.start]
        }, documentID: move.document)
        guard applied, documentID == move.document else { return applied }
        selection = move.track
        clipFocus = move.copy ? landedClip(on: move.track, at: move.start) : move.clip
        return true
    }

    /// The placement that starts at `start` on `track`. Replace mode leaves one at most.
    private func landedClip(on track: UUID, at start: Int64) -> UUID? {
        guard let snapshot, let row = (0..<snapshot.trackCount).first(where: { snapshot.trackID(at: $0) == track }) else { return nil }
        return snapshot.visibleElements(originTick: start, widthTicks: 1, originTrack: Double(row), heightTracks: 1,
                                        pixelsPerTick: 1, pixelsPerTrack: 1).elements
            .compactMap { $0.role == .clip && $0.id.track == track ? $0.id.placement : nil }
            .first { snapshot.clipMix($0)?.start == start }
    }
}
