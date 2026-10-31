import Foundation

/// The one pointer interaction in progress. Each case owns its press and its
/// preview, so two interactions cannot run at once. Tracking never writes to the
/// model; only a release commits.
enum PianoInteraction {
    case idle
    case ruler(RulerPress)
    case marquee(PianoSelectionPress)
    case insertion(PianoInsertion)
    case note(PianoGesture, preview: PianoPreview?)
    case velocity(PianoVelocityGesture, preview: PianoVelocityPreview?)
    case move(PianoMoveGesture, preview: PianoMovePreview?)
    case resize(PianoResizeGesture, preview: PianoResizePreview?)

    var isIdle: Bool { if case .idle = self { true } else { false } }
    var rulerPress: RulerPress? { if case .ruler(let press) = self { press } else { nil } }
    var selectionPress: PianoSelectionPress? { if case .marquee(let press) = self { press } else { nil } }
    var insertion: PianoInsertion? { if case .insertion(let insertion) = self { insertion } else { nil } }
    var notePreview: PianoPreview? { if case .note(_, let preview) = self { preview } else { nil } }
    var velocityPreview: PianoVelocityPreview? { if case .velocity(_, let preview) = self { preview } else { nil } }
    var movePreview: PianoMovePreview? { if case .move(_, let preview) = self { preview } else { nil } }
    var resizePreview: PianoResizePreview? { if case .resize(_, let preview) = self { preview } else { nil } }

    /// A marquee follows the right button, so left-button drags do not move it.
    var followsLeftDrag: Bool {
        switch self {
        case .idle, .marquee: false
        case .ruler, .insertion, .note, .velocity, .move, .resize: true
        }
    }

    mutating func drag(to point: CGPoint, snapping: Bool, camera: PianoCamera) {
        switch self {
        case .idle, .marquee: break
        case .ruler(var press):
            press.update(x: point.x, snapping: snapping)
            self = .ruler(press)
        case .insertion(var insertion):
            insertion.update(x: point.x, y: point.y, snapping: snapping)
            self = .insertion(insertion)
        case .note(let gesture, _):
            self = .note(gesture, preview: gesture.notePreview(at: point, camera: camera, snapping: snapping))
        case .velocity(let gesture, _):
            self = .velocity(gesture, preview: gesture.preview(y: point.y))
        case .move(let gesture, let preview):
            guard preview != nil || hypot(point.x - gesture.point.x, point.y - gesture.point.y) >= 3 else { return }
            self = .move(gesture, preview: gesture.preview(at: point, snapping: snapping))
        case .resize(let gesture, let preview):
            guard preview != nil || abs(point.x - gesture.point.x) >= 3 else { return }
            self = .resize(gesture, preview: gesture.preview(at: point, snapping: snapping))
        }
    }

    /// Group and velocity presses captured the selection they act on. A note
    /// press selects its own note, and the others do not read the selection.
    mutating func endIfBoundToSelection() {
        switch self {
        case .velocity, .move, .resize: self = .idle
        case .idle, .ruler, .marquee, .insertion, .note: break
        }
    }

    /// Keeps a single-note drag, which maps through the current camera rather
    /// than one captured at press.
    mutating func endOnResize() {
        if case .note = self { return }
        self = .idle
    }
}

extension PianoGesture {
    func notePreview(at point: CGPoint, camera: PianoCamera, snapping: Bool) -> PianoPreview? {
        preview(x: point.x, y: point.y, camera: camera, snapping: snapping).map { PianoPreview(id: id, geometry: $0) }
    }
}
