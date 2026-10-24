import EchodeBridge
import Foundation
import Metal
import QuartzCore
import Synchronization

struct CanvasInput: Sendable {
    let snapshot: Snapshot?
    let selected: UUID?
    let camera: ArrangeCamera
    let width: Double
    let height: Double
    let scale: Double
    var piano: PianoRenderInput? = nil
    var playback: PlaybackFeed? = nil
    var header: ArrangeHeaderInput? = nil
    var selectedClip: UUID? = nil
    var faderPreview: FaderPreview? = nil
    var clipDraft: ClipDraft? = nil
    var clipMove: ClipMovePreview? = nil
    /// Screen refresh in frames per second. The display link follows this
    /// instead of a fixed 60 Hz cap. 60 is the value before a window exists.
    var displayFramesPerSecond: Double = 60

    var drawableSize: CGSize { canvasDrawableSize(width: width, height: height, scale: scale) }
    func acceptsDrawable(size: CGSize) -> Bool { width > 0 && height > 0 && size == drawableSize }
}

/// A resize can overtake an acquired drawable. The current input is still
/// drawn at the same pixel positions; only the revision stays pending so a
/// later callback redraws at the corrected extent.
struct CanvasDrawablePolicy: Equatable {
    let mismatched: Bool
    let draws: Bool
    let consumesRevision: Bool

    static func resolve(input: CanvasInput, drawableSize: CGSize) -> Self {
        let mismatched = !input.acceptsDrawable(size: drawableSize)
        return Self(mismatched: mismatched, draws: true, consumesRevision: !mismatched)
    }
}

/// Pure pause decision for the display link. A stale drawable keeps the link
/// awake so the next callback can retry at the corrected extent.
func canvasLinkPaused(ready: Bool, continuous: Bool, metersActive: Bool, playing: Bool,
                      revisionDrawn: Bool, staleDrawable: Bool) -> Bool {
    !ready || (!continuous && !metersActive && !playing && revisionDrawn && !staleDrawable)
}

/// Matches the S1 spike: at least 60 Hz when the display can, otherwise the
/// display's own maximum, and never above that maximum.
public func displayFrameRateRange(maximumFramesPerSecond: Double) -> CAFrameRateRange {
    let maximum = max(1, maximumFramesPerSecond)
    let floor = min(60, maximum)
    return CAFrameRateRange(minimum: Float(floor), maximum: Float(maximum), preferred: Float(maximum))
}

private struct RenderState {
    var input: CanvasInput?
    var revision: UInt64 = 0
    var stopping = false
    var loop: CFRunLoop?
    var continuous = false
    var mainBusy = false
    var measurement = ArrangeMeasurement()
}

public struct ArrangeMeasurement: Sendable, Codable {
    public var callbacks = 0
    public var frames = 0
    public var framesOffMain = 0
    public var framesWhileMainBusy = 0
    /// Frames drawn at a stale drawable extent without consuming a revision.
    public var sizeMismatches = 0
    public var unavailableSlots = 0
    public var maxCPUSeconds = 0.0
    public var headerFrames = 0
    public var maxHeaderLaneError = 0.0
    /// Times the render thread was asked to reconsider the display link.
    public var wakeCount = 0
    /// Times the link went from paused to running on that thread.
    public var resumeCount = 0
    public var continuous = false
    /// Last pause decision on the render thread. Starts paused until a frame is due.
    public var displayLinkPaused = true
    public var revision: UInt64 = 0
    public var drawnRevision: UInt64?
    public var drawnTick: Int64?
    public var drawnPlaying: Bool?
    /// Playhead x in points. Nil when this frame did not draw one.
    public var playheadX: Double?
}

struct Instance {
    var rect: SIMD4<Float>
    var color: SIMD4<Float>
}

/// Keeps the arrange display link awake for one late audio buffer after stop.
/// A 4096-frame buffer at 48 kHz is 85 ms, and the live stop tail is 5 ms.
struct MeterLinkHold: Equatable, Sendable {
    static let duration = 0.2
    var sawPlayback = false
    var quiesceUntil = 0.0
    mutating func note(playing: Bool, now: Double) {
        if playing {
            sawPlayback = true
            quiesceUntil = 0
        } else if sawPlayback {
            sawPlayback = false
            quiesceUntil = now + Self.duration
        }
    }
    func holding(now: Double) -> Bool { quiesceUntil > now }
}

/// The display link, encoders and buffer array are confined to the render
/// thread. MainActor exchanges immutable inputs through the mutex; completion
/// handlers only release their frame slot. No AppKit view is read here.
final class CanvasRenderer: NSObject, CAMetalDisplayLinkDelegate, @unchecked Sendable {
    private let layer: CAMetalLayer
    private let device: any MTLDevice
    private let queue: any MTLCommandQueue
    private let painter: ArrangePainter
    private let state = Mutex(RenderState())
    private let slots = FrameSlots()
    private let stopped = DispatchSemaphore(value: 0)
    private let drivesDisplayLink: Bool
    private let onElements: @MainActor @Sendable ([ArrangeElement]) -> Void
    private let onPiano: (@MainActor @Sendable (PianoFrame) -> Void)?
    private let onArrange: (@MainActor @Sendable (ArrangeFrame) -> Void)?
    // Created on the render thread. The display link delivers callbacks to the
    // thread that created it and is running its run loop.
    private var link: CAMetalDisplayLink?
    // Only the render thread reads or changes these fields.
    private var buffers: [(any MTLBuffer)?] = [nil, nil, nil]
    private var appliedFrameRate: Double = 0
    private var drawnRevision: UInt64?
    private var publishedRevision: UInt64?
    private var staleDrawable = false
    private var submittedPause = true
    private var metersActive = false
    private var meterHold = MeterLinkHold()
    private var geometry = CanvasGeometryCache()

    init(layer: CAMetalLayer, onElements: @escaping @MainActor @Sendable ([ArrangeElement]) -> Void,
         onPiano: (@MainActor @Sendable (PianoFrame) -> Void)? = nil,
         onArrange: (@MainActor @Sendable (ArrangeFrame) -> Void)? = nil,
         startsDisplayLink: Bool = true) throws {
        guard let device = MTLCreateSystemDefaultDevice(), let queue = device.makeCommandQueue() else {
            throw CocoaError(.featureUnsupported)
        }
        self.painter = try ArrangePainter(device: device)
        self.layer = layer
        self.device = device
        self.queue = queue
        self.onElements = onElements
        self.onPiano = onPiano
        self.onArrange = onArrange
        self.drivesDisplayLink = startsDisplayLink
        layer.device = device
        layer.pixelFormat = .bgra8Unorm
        layer.framebufferOnly = true
        layer.maximumDrawableCount = 3
        super.init()
        if startsDisplayLink {
            let thread = Thread { [self] in run() }
            thread.name = "echode-metal"
            thread.start()
        }
    }

    func update(_ input: CanvasInput) {
        state.withLock { value in
            value.input = input
            value.revision &+= 1
            wake(value.loop)
        }
    }

    func stop() {
        state.withLock { $0.stopping = true; wake($0.loop) }
        if drivesDisplayLink { stopped.wait() }
    }

    func beginMeasurement() { state.withLock { $0.continuous = true; $0.measurement = ArrangeMeasurement(); wake($0.loop) } }
    func markMainBusy(_ busy: Bool) { state.withLock { $0.mainBusy = busy } }
    var measurement: ArrangeMeasurement {
        state.withLock { value in
            var result = value.measurement
            // Readers must see a newly requested revision before its wake runs.
            result.revision = value.revision
            return result
        }
    }

    private func wake(_ loop: CFRunLoop?) {
        guard let loop else { return }
        CFRunLoopPerformBlock(loop, CFRunLoopMode.defaultMode.rawValue) { [self] in
            state.withLock { $0.measurement.wakeCount += 1 }
            if let link {
                applyFrameRate(link)
                applyLinkPause(link)
            }
            if state.withLock({ $0.stopping }) { CFRunLoopStop(CFRunLoopGetCurrent()) }
        }
        CFRunLoopWakeUp(loop)
    }

    /// The system clamps this to the display. A 60 Hz screen stays at 60.
    private func applyFrameRate(_ link: CAMetalDisplayLink) {
        let maximum = state.withLock { $0.input?.displayFramesPerSecond ?? 60 }
        if maximum == appliedFrameRate { return }
        appliedFrameRate = maximum
        link.preferredFrameRateRange = displayFrameRateRange(maximumFramesPerSecond: maximum)
    }

    /// Writes pause only when it changes. Assigning the same value from the
    /// callback or the run-loop turn drops the next vsync.
    private func applyLinkPause(_ link: CAMetalDisplayLink) {
        setLinkPause(link, paused: publishLinkState())
    }

    private func setLinkPause(_ link: CAMetalDisplayLink, paused: Bool) {
        if link.isPaused != paused { link.isPaused = paused }
    }

    /// Pause decision for the render thread. A stopped frame that already
    /// matches the requested revision lets the link sleep; play and a new
    /// revision wake it without drawing every idle vsync.
    private func publishLinkState() -> Bool {
        state.withLock { value in
            let ready = value.input.map { $0.width > 0 && $0.height > 0 } ?? false
            let paused = canvasLinkPaused(ready: ready, continuous: value.continuous, metersActive: metersActive,
                                          playing: value.input?.playback?.position.playing == true,
                                          revisionDrawn: drawnRevision == value.revision,
                                          staleDrawable: staleDrawable)
            let wasPaused = value.measurement.displayLinkPaused
            value.measurement.displayLinkPaused = paused
            value.measurement.continuous = value.continuous
            value.measurement.revision = value.revision
            value.measurement.drawnRevision = drawnRevision
            if !paused && wasPaused { value.measurement.resumeCount += 1 }
            return paused
        }
    }

    private func run() {
        let link = CAMetalDisplayLink(metalLayer: layer)
        self.link = link
        link.delegate = self
        applyFrameRate(link)
        link.add(to: .current, forMode: .common)
        let keepAlive = Port()
        RunLoop.current.add(keepAlive, forMode: .default)
        // Publish the loop only after the link can accept a wake.
        state.withLock { $0.loop = CFRunLoopGetCurrent() }
        while !state.withLock({ $0.stopping }) {
            applyLinkPause(link)
            autoreleasepool { _ = RunLoop.current.run(mode: .default, before: .distantFuture) }
        }
        link.invalidate()
        self.link = nil
        keepAlive.invalidate()
        state.withLock { $0.loop = nil }
        stopped.signal()
    }

    func metalDisplayLink(_ link: CAMetalDisplayLink, needsUpdate update: CAMetalDisplayLink.Update) {
        applyFrameRate(link)
        submit(texture: update.drawable.texture) { $0.present(update.drawable) }
        setLinkPause(link, paused: submittedPause)
    }

    /// The display callback and offscreen regressions use this entire submission
    /// path. Only drawable presentation and display-link scheduling are omitted
    /// offscreen; encoding, revision consumption and pause publication are shared.
    /// Call on the renderer's owning thread; GPU completion never blocks here.
    @discardableResult
    func submit(texture: any MTLTexture, present: ((any MTLCommandBuffer) -> Void)? = nil) -> (any MTLCommandBuffer)? {
        // Publish once and apply that same decision to the real display link.
        // Reconsidering here could count an unpaused link as resumed if a new
        // input arrived between the two decisions.
        defer { submittedPause = publishLinkState() }
        state.withLock { $0.measurement.callbacks += 1 }
        let (input, revision, stopping, continuous, frame) = state.withLock {
            ($0.input, $0.revision, $0.stopping, $0.continuous, $0.measurement.frames)
        }
        // Every callback owns a drawable. Leaving it unpresented stops later
        // callbacks, including the next play or locate. Clear skipped frames:
        // a drawable from the pool does not contain the previous picture.
        guard !stopping, let input, input.width > 0, input.height > 0 else {
            return presentBackground(texture: texture, present: present)
        }
        // Draw a stale extent at the input's pixel positions and leave the
        // revision pending; the next callback retries at the corrected size.
        let submission = CanvasDrawablePolicy.resolve(input: input,
            drawableSize: CGSize(width: texture.width, height: texture.height))
        staleDrawable = submission.mismatched
        guard submission.draws else {
            return presentBackground(texture: texture, present: present)
        }
        guard let slot = slots.acquire() else {
            state.withLock { $0.measurement.unavailableSlots += 1 }
            return presentBackground(texture: texture, present: present)
        }
        let started = CACurrentMediaTime()
        var committed = false
        defer { if !committed { slots.release(slot) } }
        var camera = input.camera
        if continuous {
            camera.originBeat += Double(frame % 24) / 48
            if input.header != nil {
                camera.originTrack += Double(frame % 24) / 48
                camera.constrain(trackCount: input.snapshot?.trackCount ?? 0, height: input.height)
            }
        }
        let (arranged, piano) = geometry.frames(input: input, camera: camera, revision: revision, continuous: continuous)
        let now = CACurrentMediaTime()
        meterHold.note(playing: input.playback?.position.playing == true, now: now)
        let peaksLive = arranged?.headers.contains { $0.peak > MixScale.meterSilence } ?? false
        metersActive = peaksLive || meterHold.holding(now: now)
        var instances = piano?.gridInstances ?? arranged?.timelineInstances ?? []
        let timelineCount = instances.count
        let headerInstances = piano.map { $0.degreeInstances + $0.keyboardInstances + $0.rulerInstances } ?? arranged?.headerInstances ?? []
        instances.append(contentsOf: headerInstances)
        let length = max(1, instances.count) * MemoryLayout<Instance>.stride
        if (buffers[slot]?.length ?? 0) < length { buffers[slot] = device.makeBuffer(length: length, options: .storageModeShared) }
        guard let buffer = buffers[slot], let command = queue.makeCommandBuffer() else {
            return presentBackground(texture: texture, present: present)
        }
        instances.withUnsafeBytes { bytes in
            if let source = bytes.baseAddress { buffer.contents().copyMemory(from: source, byteCount: bytes.count) }
        }
        let pass = MTLRenderPassDescriptor()
        pass.colorAttachments[0].texture = texture
        pass.colorAttachments[0].loadAction = .clear
        pass.colorAttachments[0].storeAction = .store
        pass.colorAttachments[0].clearColor = CanvasBackground.clearColor
        guard let encoder = command.makeRenderCommandEncoder(descriptor: pass) else {
            return presentBackground(texture: texture, present: present)
        }
        painter.encode(encoder: encoder, texture: texture, buffer: buffer,
                       timelineCount: timelineCount, headerCount: headerInstances.count, frame: arranged, piano: piano, slot: slot)
        encoder.endEncoding()
        let slots = slots
        command.addCompletedHandler { _ in slots.release(slot) }
        present?(command)
        command.commit()
        committed = true
        let feedbackChanged = continuous || publishedRevision != revision
        publishedRevision = revision
        if submission.consumesRevision { drawnRevision = revision }
        state.withLock { value in
            if submission.mismatched { value.measurement.sizeMismatches += 1 }
            value.measurement.frames += 1
            if !Thread.isMainThread { value.measurement.framesOffMain += 1 }
            if value.mainBusy { value.measurement.framesWhileMainBusy += 1 }
            value.measurement.maxCPUSeconds = max(value.measurement.maxCPUSeconds, CACurrentMediaTime() - started)
            value.measurement.drawnTick = arranged?.playheadTick ?? piano?.playheadTick
            value.measurement.drawnPlaying = arranged?.playheadPlaying ?? piano?.playheadPlaying
            value.measurement.playheadX = arranged?.playheadX ?? piano?.playheadX
            if let arranged, input.header != nil {
                value.measurement.headerFrames += 1
                let lanes = arranged.elements.filter { $0.role == .track }
                for (header, lane) in zip(arranged.headers, lanes) {
                    value.measurement.maxHeaderLaneError = max(value.measurement.maxHeaderLaneError, Double(abs(header.y - lane.y)))
                }
            }
        }
        // A moving playhead does not change hit regions or accessibility.
        // Publish geometry only when inputs change (or the explicit probe moves it).
        if feedbackChanged {
            Task { @MainActor [onElements, onPiano, onArrange] in
                if let piano { onPiano?(piano) }
                else if let arranged {
                    if let onArrange { onArrange(arranged) } else { onElements(arranged.elements) }
                }
            }
        }
        return command
    }

    /// Returns a defined background, including during zero-size layout and
    /// shutdown. An unrendered drawable can contain stale pool contents.
    private func presentBackground(texture: any MTLTexture,
                                   present: ((any MTLCommandBuffer) -> Void)?) -> (any MTLCommandBuffer)? {
        guard let command = queue.makeCommandBuffer() else { return nil }
        let pass = MTLRenderPassDescriptor()
        pass.colorAttachments[0].texture = texture
        pass.colorAttachments[0].loadAction = .clear
        pass.colorAttachments[0].storeAction = .store
        pass.colorAttachments[0].clearColor = CanvasBackground.clearColor
        guard let encoder = command.makeRenderCommandEncoder(descriptor: pass) else { return nil }
        encoder.endEncoding()
        present?(command)
        command.commit()
        return command
    }
}

let arrangeShader = """
#include <metal_stdlib>
using namespace metal;
struct Instance { float4 rect; float4 color; };
struct Vertex { float4 position [[position]]; float4 color; };
vertex Vertex arrange_vertex(uint v [[vertex_id]], uint i [[instance_id]],
                            constant Instance *instances [[buffer(0)]], constant float2 &size [[buffer(1)]]) {
    const float2 corners[6] = {float2(0,0),float2(1,0),float2(0,1),float2(1,0),float2(1,1),float2(0,1)};
    float2 p = instances[i].rect.xy + corners[v] * instances[i].rect.zw;
    return {float4(p.x / size.x * 2 - 1, 1 - p.y / size.y * 2, 0, 1), instances[i].color};
}
fragment float4 arrange_fragment(Vertex input [[stage_in]]) { return input.color; }
struct Wave {
    float4 rect;
    float4 color;
    uint offset;
    uint columns;
    uint pad0;
    uint pad1;
};
struct WaveVertex {
    float4 position [[position]];
    float2 uv;
    float4 color;
    float height;
    uint offset [[flat]];
    uint columns [[flat]];
};
vertex WaveVertex wave_vertex(uint v [[vertex_id]], uint i [[instance_id]],
                            constant Wave *waves [[buffer(0)]], constant float2 &size [[buffer(1)]]) {
    const float2 corners[6] = {float2(0,0),float2(1,0),float2(0,1),float2(1,0),float2(1,1),float2(0,1)};
    float2 p = waves[i].rect.xy + corners[v] * waves[i].rect.zw;
    WaveVertex out;
    out.position = float4(p.x / size.x * 2 - 1, 1 - p.y / size.y * 2, 0, 1);
    out.uv = corners[v];
    out.color = waves[i].color;
    out.height = waves[i].rect.w;
    out.offset = waves[i].offset;
    out.columns = waves[i].columns;
    return out;
}
fragment float4 wave_fragment(WaveVertex input [[stage_in]], constant float2 *peaks [[buffer(0)]]) {
    if (input.columns == 0) discard_fragment();
    uint column = min(uint(input.uv.x * float(input.columns)), input.columns - 1);
    float2 envelope = peaks[input.offset + column];
    float amplitude = (0.5 - input.uv.y) * 2.0;
    float pixel = 1.0 / max(input.height, 1.0);
    if (amplitude + pixel < envelope.x || amplitude - pixel > envelope.y) discard_fragment();
    return input.color;
}
struct LabelVertex { float4 position [[position]]; float2 uv; float4 color; };
vertex LabelVertex label_vertex(uint v [[vertex_id]], constant Instance &label [[buffer(0)]],
                               constant float2 &size [[buffer(1)]]) {
    const float2 corners[6] = {float2(0,0),float2(1,0),float2(0,1),float2(1,0),float2(1,1),float2(0,1)};
    float2 p = label.rect.xy + corners[v] * label.rect.zw;
    return {float4(p.x / size.x * 2 - 1, 1 - p.y / size.y * 2, 0, 1), corners[v], label.color};
}
fragment float4 label_fragment(LabelVertex input [[stage_in]], texture2d<float> label [[texture(0)]]) {
    constexpr sampler s(coord::normalized, filter::linear, address::clamp_to_edge);
    return float4(input.color.rgb, input.color.a * label.sample(s, input.uv).r);
}
"""
