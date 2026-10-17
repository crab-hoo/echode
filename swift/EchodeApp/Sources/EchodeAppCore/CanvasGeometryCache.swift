import Foundation

/// Confined to the render thread. An input revision changes whenever the view
/// publishes a new immutable input; playback position and peaks stay live.
struct CanvasGeometryCache {
    private var revision: UInt64?
    private var camera: ArrangeCamera?
    private var arranged: ArrangeFrame?
    private var piano: PianoFrame?
    private(set) var builds = 0

    mutating func frames(input: CanvasInput, camera: ArrangeCamera, revision: UInt64,
                         continuous: Bool = false) -> (arranged: ArrangeFrame?, piano: PianoFrame?) {
        if self.revision != revision || self.camera != camera || continuous {
            arranged = input.piano == nil ? ArrangeFrame(input: input, camera: camera, revision: revision) : nil
            piano = input.piano.map { PianoFrame(input: input, piano: $0, revision: revision) }
            self.revision = revision
            self.camera = camera
            builds += 1
        }
        var current = arranged
        current?.refreshMeters()
        return (current, piano)
    }
}
