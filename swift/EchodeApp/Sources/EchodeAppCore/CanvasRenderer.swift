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
    private let onElements: @MainActor @Sendable ([ArrangeElement]) -> Void
    private let onPiano: (@MainActor @Sendable (PianoFrame) -> Void)?
    private let onArrange: (@MainActor @Sendable (ArrangeFrame) -> Void)?
    // Created on the render thread. The display link delivers callbacks to the
    // thread that created it and is running its run loop.
    private var link: CAMetalDisplayLink?
    // Only the render thread reads or changes these fields.
    private var buffers: [(any MTLBuffer)?] = [nil, nil, nil]
    private var drawnRevision: UInt64?

    init(layer: CAMetalLayer, size: CGSize, scale: CGFloat, onElements: @escaping @MainActor @Sendable ([ArrangeElement]) -> Void,
         onPiano: (@MainActor @Sendable (PianoFrame) -> Void)? = nil,
         onArrange: (@MainActor @Sendable (ArrangeFrame) -> Void)? = nil) throws {
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
        layer.device = device
        layer.pixelFormat = .bgra8Unorm
        layer.framebufferOnly = true
        layer.maximumDrawableCount = 3
        layer.contentsScale = scale
        layer.drawableSize = CGSize(width: max(1, size.width * scale), height: max(1, size.height * scale))
        super.init()
        let thread = Thread { [self] in run() }
        thread.name = "echode-metal"
        thread.start()
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
        stopped.wait()
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
            if let input = state.withLock({ $0.input }) {
                // Apply between display-link callbacks, after any preceding
                // frame was submitted, so layer sizing is confined too.
                layer.contentsScale = input.scale
                layer.drawableSize = CGSize(width: max(1, input.width * input.scale), height: max(1, input.height * input.scale))
            }
            if let link { applyLinkPause(link) }
            if state.withLock({ $0.stopping }) { CFRunLoopStop(CFRunLoopGetCurrent()) }
        }
        CFRunLoopWakeUp(loop)
    }

    /// Writes pause only when it changes. Assigning the same value from the
    /// callback or the run-loop turn drops the next vsync.
    private func applyLinkPause(_ link: CAMetalDisplayLink) {
        let paused = publishLinkState()
        if link.isPaused != paused { link.isPaused = paused }
    }

    /// Pause decision for the render thread. A stopped frame that already
    /// matches the requested revision lets the link sleep; play and a new
    /// revision wake it without drawing every idle vsync.
    private func publishLinkState() -> Bool {
        state.withLock { value in
            let ready = value.input.map { $0.width > 0 && $0.height > 0 } ?? false
            let paused = !ready || (!value.continuous && value.input?.playback?.position.playing != true && drawnRevision == value.revision)
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
        link.preferredFrameRateRange = CAFrameRateRange(minimum: 60, maximum: 60, preferred: 60)
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
        state.withLock { $0.measurement.callbacks += 1 }
        let (input, revision, stopping, continuous, frame) = state.withLock {
            ($0.input, $0.revision, $0.stopping, $0.continuous, $0.measurement.frames)
        }
        // Every callback owns a drawable. Leaving it unpresented stops later
        // callbacks, including the next play or locate. Clear skipped frames:
        // a drawable from the pool does not contain the previous picture.
        guard !stopping, let input, input.width > 0, input.height > 0 else {
            present(update.drawable)
            applyLinkPause(link)
            return
        }
        guard let slot = slots.acquire() else {
            state.withLock { $0.measurement.unavailableSlots += 1 }
            present(update.drawable)
            return
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
        let arranged = input.piano == nil ? ArrangeFrame(input: input, camera: camera, revision: revision) : nil
        let piano = input.piano.map { PianoFrame(input: input, piano: $0, revision: revision) }
        var instances = piano?.gridInstances ?? arranged?.timelineInstances ?? []
        let timelineCount = instances.count
        let headerInstances = piano.map { $0.degreeInstances + $0.keyboardInstances + $0.rulerInstances } ?? arranged?.headerInstances ?? []
        instances.append(contentsOf: headerInstances)
        let length = max(1, instances.count) * MemoryLayout<Instance>.stride
        if (buffers[slot]?.length ?? 0) < length { buffers[slot] = device.makeBuffer(length: length, options: .storageModeShared) }
        guard let buffer = buffers[slot], let command = queue.makeCommandBuffer() else {
            present(update.drawable)
            return
        }
        instances.withUnsafeBytes { bytes in
            if let source = bytes.baseAddress { buffer.contents().copyMemory(from: source, byteCount: bytes.count) }
        }
        let pass = MTLRenderPassDescriptor()
        pass.colorAttachments[0].texture = update.drawable.texture
        pass.colorAttachments[0].loadAction = .clear
        pass.colorAttachments[0].storeAction = .store
        pass.colorAttachments[0].clearColor = MTLClearColor(red: 0.09, green: 0.10, blue: 0.12, alpha: 1)
        guard let encoder = command.makeRenderCommandEncoder(descriptor: pass) else {
            present(update.drawable)
            return
        }
        painter.encode(encoder: encoder, texture: update.drawable.texture, buffer: buffer,
                       timelineCount: timelineCount, headerCount: headerInstances.count, frame: arranged, piano: piano)
        encoder.endEncoding()
        let slots = slots
        command.addCompletedHandler { _ in slots.release(slot) }
        command.present(update.drawable)
        command.commit()
        committed = true
        let feedbackChanged = continuous || drawnRevision != revision
        drawnRevision = revision
        state.withLock { value in
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
        // A stopped frame has consumed this revision. Pause on this run loop
        // so the next play or locate is what wakes the link, not a poll.
        applyLinkPause(link)
        // A moving playhead does not change hit regions or accessibility.
        // Publish geometry only when inputs change (or the explicit probe moves it).
        guard feedbackChanged else { return }
        Task { @MainActor [onElements, onPiano, onArrange] in
            if let piano { onPiano?(piano) }
            else if let arranged {
                if let onArrange { onArrange(arranged) } else { onElements(arranged.elements) }
            }
        }
    }

    /// Returns a defined background, including during zero-size layout and
    /// shutdown. An unrendered drawable can contain stale pool contents.
    private func present(_ drawable: any CAMetalDrawable) {
        guard let command = queue.makeCommandBuffer() else { return }
        let pass = MTLRenderPassDescriptor()
        pass.colorAttachments[0].texture = drawable.texture
        pass.colorAttachments[0].loadAction = .clear
        pass.colorAttachments[0].storeAction = .store
        pass.colorAttachments[0].clearColor = MTLClearColor(red: 0.09, green: 0.10, blue: 0.12, alpha: 1)
        guard let encoder = command.makeRenderCommandEncoder(descriptor: pass) else { return }
        encoder.endEncoding()
        command.present(drawable)
        command.commit()
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
