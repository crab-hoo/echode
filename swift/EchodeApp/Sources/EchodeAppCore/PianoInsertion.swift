import EchodeBridge
import Foundation

/// A blank-grid press owns its start, click length and camera until release.
/// No model writes occur during tracking.
struct PianoInsertion: Sendable {
    let camera: PianoCamera
    let x: Double
    let y: Double
    let original: NoteGeometry
    private(set) var dragged = false
    private(set) var geometry: NoteGeometry?

    init?(x: Double, y: Double, camera: PianoCamera, snapping: Bool, length: Int64? = nil) {
        guard x.isFinite, y.isFinite, camera.grid > 0,
            camera.originBeat.isFinite, camera.topPitch.isFinite,
            camera.pixelsPerBeat.isFinite, camera.pixelsPerBeat > 0,
            camera.pixelsPerPitch.isFinite, camera.pixelsPerPitch > 0,
            let start = camera.rulerTick(at: x, snapping: snapping),
            let pitch = Self.pitch(at: y, camera: camera)
        else { return nil }
        let original = NoteGeometry(start: start, length: length ?? camera.grid, pitch: pitch)
        guard original.isValid else { return nil }
        self.camera = camera
        self.x = x
        self.y = y
        self.original = original
        geometry = original
    }

    mutating func update(x: Double, y: Double, snapping: Bool) {
        guard x.isFinite, y.isFinite else {
            geometry = nil
            return
        }
        if hypot(x - self.x, y - self.y) >= 3 { dragged = true }
        guard dragged else {
            geometry = original
            return
        }
        guard let end = camera.rulerTick(at: x, snapping: snapping),
            let pitch = Self.pitch(at: y, camera: camera)
        else {
            geometry = nil
            return
        }
        // New notes keep a grid-cell minimum even with Shift, unlike resizing
        // existing notes. Moving left never moves the pressed start (ADR 0117).
        let value = NoteGeometry(start: original.start, length: max(camera.grid, end - original.start), pitch: pitch)
        geometry = value.isValid ? value : nil
    }

    private static func pitch(at y: Double, camera: PianoCamera) -> Int? {
        let value = (camera.topPitch - y / camera.pixelsPerPitch).rounded(.up)
        guard value.isFinite else { return nil }
        return Int(min(Double(ModelLimits.midi7.upperBound), max(0, value)))
    }
}
