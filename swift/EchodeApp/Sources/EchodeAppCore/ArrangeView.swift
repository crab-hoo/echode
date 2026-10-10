import AppKit
import EchodeBridge
import Metal
import QuartzCore
import SwiftUI
import Synchronization

public struct ArrangeCanvas: NSViewRepresentable {
    public let snapshot: Snapshot
    @Binding public var selection: UUID?
    public var playback: PlaybackFeed?
    public var tick: Int64

    public init(snapshot: Snapshot, selection: Binding<UUID?>, playback: PlaybackFeed? = nil, tick: Int64 = 0) {
        self.snapshot = snapshot
        _selection = selection
        self.playback = playback
        self.tick = tick
    }

    public func makeNSView(context: Context) -> ArrangeView {
        ArrangeView(frame: .zero)
    }

    public func updateNSView(_ view: ArrangeView, context: Context) {
        view.onSelect = { selection = $0 }
        view.setSnapshot(snapshot, selected: selection)
        view.setPlayback(playback)
    }

    public static func dismantleNSView(_ view: ArrangeView, coordinator: ()) { view.stop() }
}

@MainActor
public final class ArrangeView: NSView {
    private let metalLayer = CAMetalLayer()
    private var renderer: CanvasRenderer?
    private var snapshot: Snapshot?
    private var selected: UUID?
    private var elements: [ArrangeElement] = []
    // AppKit requires the view to retain virtual elements while they are shown.
    private var accessibleElements: [ArrangeElement.ID: NSAccessibilityElement] = [:]
    private var camera = ArrangeCamera()
    private var playback: PlaybackFeed?
    var onSelect: ((UUID) -> Void)?

    public override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        setAccessibilityElement(true)
        setAccessibilityRole(.group)
        setAccessibilityLabel(text("Arrange"))
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }
    public override var isFlipped: Bool { true }
    public override var acceptsFirstResponder: Bool { true }
    public override func makeBackingLayer() -> CALayer { metalLayer }

    public override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        guard window != nil else { stop(); return }
        if renderer == nil {
            do {
                renderer = try CanvasRenderer(layer: metalLayer, size: bounds.size, scale: window?.backingScaleFactor ?? 1) { [weak self] shown in
                    guard let self else { return }
                    self.elements = shown
                    NSAccessibility.post(element: self, notification: .layoutChanged)
                }
            } catch {
                NSLog("Echode arrange renderer: %@", String(describing: error))
                let label = NSTextField(labelWithString: text("Metal is unavailable"))
                label.frame = bounds
                label.autoresizingMask = [.width, .height]
                addSubview(label)
            }
        }
        refresh()
    }

    public func setSnapshot(_ snapshot: Snapshot, selected: UUID?) {
        guard self.snapshot !== snapshot || self.selected != selected else { return }
        self.snapshot = snapshot
        self.selected = selected
        refresh()
    }

    public func setPlayback(_ playback: PlaybackFeed?) { self.playback = playback; refresh() }

    public override func layout() { super.layout(); refresh() }
    public override func viewDidChangeBackingProperties() { super.viewDidChangeBackingProperties(); refresh() }

    public func stop() { renderer?.stop(); renderer = nil }

    // Explicit, hardware-only probe; the app never enables continuous redraw
    // or blocks MainActor during normal editing.
    public func beginMeasurement() { renderer?.beginMeasurement() }
    public func markMainBusy(_ busy: Bool) { renderer?.markMainBusy(busy) }
    public var measurement: ArrangeMeasurement { renderer?.measurement ?? ArrangeMeasurement() }
    public var rendererAvailable: Bool { renderer != nil }

    public override func scrollWheel(with event: NSEvent) {
        camera.originBeat = max(0, camera.originBeat - Double(event.scrollingDeltaX) / camera.pixelsPerBeat)
        camera.originTrack = max(0, camera.originTrack - Double(event.scrollingDeltaY) / camera.pixelsPerTrack)
        refresh()
    }

    public override func magnify(with event: NSEvent) {
        let factor = max(0.1, 1 + Double(event.magnification))
        if event.modifierFlags.contains(.option) || event.modifierFlags.contains(.shift) {
            camera.pixelsPerTrack = min(240, max(16, camera.pixelsPerTrack * factor))
        }
        if !event.modifierFlags.contains(.option) || event.modifierFlags.contains(.shift) {
            camera.pixelsPerBeat = min(1200, max(8, camera.pixelsPerBeat * factor))
        }
        refresh()
    }

    public override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        let point = convert(event.locationInWindow, from: nil)
        if let hit = elements.last(where: {
            NSRect(x: CGFloat($0.x), y: CGFloat($0.y), width: CGFloat($0.width), height: CGFloat($0.height)).contains(point)
        }) { onSelect?(hit.id.track) }
    }

    public override func accessibilityChildren() -> [Any]? {
        guard let window else { accessibleElements = [:]; return [] }
        var visible: [ArrangeElement.ID: NSAccessibilityElement] = [:]
        let children = elements.map { element in
            let local = NSRect(x: CGFloat(element.x), y: CGFloat(element.y),
                               width: CGFloat(element.width), height: CGFloat(element.height))
            let screen = window.convertToScreen(convert(local, to: nil))
            let label: String
            switch element.role {
            case .track: label = text("Track")
            case .clip: label = text("Clip")
            case .note: label = text("Note")
            }
            let child = accessibleElements[element.id] ?? NSAccessibilityElement()
            child.setAccessibilityElement(true)
            child.setAccessibilityRole(.staticText)
            child.setAccessibilityFrame(screen)
            child.setAccessibilityLabel(label)
            child.setAccessibilityParent(self)
            child.setAccessibilityIdentifier("\(element.id.track)/\(element.id.placement?.uuidString ?? "")/\(element.id.note?.uuidString ?? "")")
            visible[element.id] = child
            return child
        }
        accessibleElements = visible
        return children
    }

    private func refresh() {
        let scale = window?.backingScaleFactor ?? 1
        renderer?.update(ArrangeInput(snapshot: snapshot, selected: selected, camera: camera,
                                      width: Double(bounds.width), height: Double(bounds.height), scale: Double(scale), playback: playback))
    }
}

struct ArrangeCamera: Sendable {
    var originBeat = 0.0
    var originTrack = 0.0
    var pixelsPerBeat = 72.0
    var pixelsPerTrack = 64.0
}

struct ArrangeInput: Sendable {
    let snapshot: Snapshot?
    let selected: UUID?
    let camera: ArrangeCamera
    let width: Double
    let height: Double
    let scale: Double
    var piano: PianoRenderInput? = nil
    var playback: PlaybackFeed? = nil
}

private struct RenderState {
    var input: ArrangeInput?
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
}

private struct Instance {
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
    private let pipeline: any MTLRenderPipelineState
    private let state = Mutex(RenderState())
    private let slots = FrameSlots()
    private let onElements: @MainActor @Sendable ([ArrangeElement]) -> Void
    private let onPiano: (@MainActor @Sendable ([PianoNote], Snapshot?) -> Void)?
    private let link: CAMetalDisplayLink
    // Only the render thread reads or changes these fields.
    private var buffers: [(any MTLBuffer)?] = [nil, nil, nil]
    private var drawnRevision: UInt64?

    init(layer: CAMetalLayer, size: CGSize, scale: CGFloat, onElements: @escaping @MainActor @Sendable ([ArrangeElement]) -> Void,
         onPiano: (@MainActor @Sendable ([PianoNote], Snapshot?) -> Void)? = nil) throws {
        guard let device = MTLCreateSystemDefaultDevice(), let queue = device.makeCommandQueue() else {
            throw CocoaError(.featureUnsupported)
        }
        let library = try device.makeLibrary(source: arrangeShader, options: nil)
        let descriptor = MTLRenderPipelineDescriptor()
        descriptor.vertexFunction = library.makeFunction(name: "arrange_vertex")
        descriptor.fragmentFunction = library.makeFunction(name: "arrange_fragment")
        descriptor.colorAttachments[0].pixelFormat = .bgra8Unorm
        self.pipeline = try device.makeRenderPipelineState(descriptor: descriptor)
        self.layer = layer
        self.device = device
        self.queue = queue
        self.onElements = onElements
        self.onPiano = onPiano
        layer.device = device
        layer.pixelFormat = .bgra8Unorm
        layer.framebufferOnly = true
        layer.maximumDrawableCount = 3
        layer.contentsScale = scale
        layer.drawableSize = CGSize(width: max(1, size.width * scale), height: max(1, size.height * scale))
        self.link = CAMetalDisplayLink(metalLayer: layer)
        super.init()
        let thread = Thread { [self] in run() }
        thread.name = "echode-metal"
        thread.start()
    }

    func update(_ input: ArrangeInput) {
        state.withLock { value in
            value.input = input
            value.revision &+= 1
            wake(value.loop)
        }
    }

    func stop() { state.withLock { $0.stopping = true; wake($0.loop) } }

    func beginMeasurement() { state.withLock { $0.continuous = true; $0.measurement = ArrangeMeasurement(); wake($0.loop) } }
    func markMainBusy(_ busy: Bool) { state.withLock { $0.mainBusy = busy } }
    var measurement: ArrangeMeasurement { state.withLock { $0.measurement } }

    private func wake(_ loop: CFRunLoop?) {
        guard let loop else { return }
        CFRunLoopPerformBlock(loop, CFRunLoopMode.defaultMode.rawValue) { [self] in
            let (stopping, paused, input) = state.withLock {
                ($0.stopping, !$0.continuous && $0.input?.playback?.position.playing != true && drawnRevision == $0.revision, $0.input)
            }
            if let input {
                // Apply between display-link callbacks, after any preceding
                // frame was submitted, so layer sizing is confined too.
                layer.contentsScale = input.scale
                layer.drawableSize = CGSize(width: max(1, input.width * input.scale), height: max(1, input.height * input.scale))
            }
            link.isPaused = paused
            if stopping { CFRunLoopStop(CFRunLoopGetCurrent()) }
        }
        CFRunLoopWakeUp(loop)
    }

    private func run() {
        state.withLock { $0.loop = CFRunLoopGetCurrent() }
        let keepAlive = Port()
        RunLoop.current.add(keepAlive, forMode: .default)
        link.delegate = self
        link.preferredFrameRateRange = CAFrameRateRange(minimum: 60, maximum: 60, preferred: 60)
        link.add(to: .current, forMode: .common)
        while !state.withLock({ $0.stopping }) {
            link.isPaused = state.withLock { !$0.continuous && $0.input?.playback?.position.playing != true && drawnRevision == $0.revision }
            autoreleasepool { _ = RunLoop.current.run(mode: .default, before: .distantFuture) }
        }
        link.invalidate()
        keepAlive.invalidate()
        state.withLock { $0.loop = nil }
    }

    private func pianoInstances(_ piano: PianoRenderInput, input: ArrangeInput) -> ([PianoNote], [Instance], Int) {
        let camera = piano.camera
        let shown = input.snapshot?.pianoNotes(track: piano.track,
            originTick: PianoCamera.ticks(camera.originBeat) ?? 0,
            widthTicks: PianoCamera.ticks(input.width / camera.pixelsPerBeat) ?? 1,
            topPitch: camera.topPitch, heightPitches: input.height / camera.pixelsPerPitch,
            pixelsPerTick: camera.pixelsPerBeat / 705_600, pixelsPerPitch: camera.pixelsPerPitch) ?? []
        var instances: [Instance] = []
        for pitch in 0...127 where Double(pitch) < camera.topPitch + 1 && Double(pitch) > camera.topPitch - input.height / camera.pixelsPerPitch {
            let y = Float((camera.topPitch - Double(pitch)) * camera.pixelsPerPitch)
            let black = [1, 3, 6, 8, 10].contains(pitch % 12)
            let color: SIMD4<Float> = piano.key.inScale(pitch) ? SIMD4(0.14, 0.18, 0.21, 1) : (black ? SIMD4(0.09, 0.10, 0.12, 1) : SIMD4(0.12, 0.13, 0.15, 1))
            instances.append(Instance(rect: SIMD4(0, y, Float(input.width), Float(camera.pixelsPerPitch - 1)) * Float(input.scale), color: color))
        }
        let backgrounds = instances.count
        for note in shown {
            let selected = note.id == piano.focus
            instances.append(Instance(rect: SIMD4(note.x, note.y + 1, max(2, note.width), max(2, note.height - 2)) * Float(input.scale),
                                      color: selected ? SIMD4(0.99, 0.73, 0.31, 1) : SIMD4(0.42, 0.76, 0.85, 1)))
        }
        if let preview = piano.preview {
            let g = preview.geometry
            let rect = SIMD4(Float((Double(g.start) / 705_600 - camera.originBeat) * camera.pixelsPerBeat),
                             Float((camera.topPitch - Double(g.pitch)) * camera.pixelsPerPitch + 1),
                             Float(Double(g.length) / 705_600 * camera.pixelsPerBeat), Float(camera.pixelsPerPitch - 2))
            instances.append(Instance(rect: rect * Float(input.scale), color: SIMD4(0.96, 0.87, 0.54, 1)))
        }
        return (shown, instances, backgrounds)
    }

    func metalDisplayLink(_ link: CAMetalDisplayLink, needsUpdate update: CAMetalDisplayLink.Update) {
        state.withLock { $0.measurement.callbacks += 1 }
        let (input, revision, stopping, continuous, frame) = state.withLock {
            ($0.input, $0.revision, $0.stopping, $0.continuous, $0.measurement.frames)
        }
        guard !stopping, let input, input.width > 0, input.height > 0,
              continuous || input.playback?.position.playing == true || revision != drawnRevision else { return }
        guard let slot = slots.acquire() else { state.withLock { $0.measurement.unavailableSlots += 1 }; return }
        let started = CACurrentMediaTime()
        var committed = false
        defer { if !committed { slots.release(slot) } }
        var camera = input.camera
        if continuous { camera.originBeat += Double(frame % 24) / 48 }
        let shown = input.piano == nil ? input.snapshot?.visibleElements(
            originTick: Int64((camera.originBeat * 705_600).rounded()),
            widthTicks: Int64((input.width / camera.pixelsPerBeat * 705_600).rounded(.up)),
            originTrack: camera.originTrack, heightTracks: input.height / camera.pixelsPerTrack,
            pixelsPerTick: camera.pixelsPerBeat / 705_600, pixelsPerTrack: camera.pixelsPerTrack
        ).elements ?? [] : []
        // Put lane backgrounds first so grid lines can sit behind objects.
        let lanes = shown.filter { $0.role == .track }
        let objects = shown.filter { $0.role != .track }
        var instances = (lanes + objects).map { element in
            let color: SIMD4<Float>
            switch element.role {
            case .track: color = element.id.track == input.selected ? SIMD4(0.18, 0.23, 0.30, 1) : SIMD4(0.12, 0.13, 0.16, 1)
            case .clip: color = SIMD4(0.18, 0.36, 0.47, 1)
            case .note: color = SIMD4(0.52, 0.83, 0.88, 1)
            }
            return Instance(rect: SIMD4(element.x, element.y, element.width, max(1, element.height - 1)) * Float(input.scale), color: color)
        }
        let piano = input.piano.map { pianoInstances($0, input: input) }
        if let piano { instances = piano.1; camera.originBeat = input.piano!.camera.originBeat; camera.pixelsPerBeat = input.piano!.camera.pixelsPerBeat }
        let firstBeat = Int(camera.originBeat.rounded(.up))
        let lastBeat = Int((camera.originBeat + input.width / camera.pixelsPerBeat).rounded(.up))
        var beatLines: [Instance] = []
        for beat in firstBeat...max(firstBeat, lastBeat) {
            let x = Float((Double(beat) - camera.originBeat) * camera.pixelsPerBeat * input.scale)
            beatLines.append(Instance(rect: SIMD4(x, 0, Float(input.scale), Float(input.height * input.scale)),
                                      color: beat % 4 == 0 ? SIMD4(0.27, 0.29, 0.33, 1) : SIMD4(0.18, 0.20, 0.23, 1)))
        }
        let backgroundCount = piano?.2 ?? lanes.count
        instances.insert(contentsOf: beatLines, at: backgroundCount)
        if let position = input.playback?.position {
            let x = (Double(position.tick) / 705_600 - camera.originBeat) * camera.pixelsPerBeat
            if x >= 0, x < input.width {
                instances.append(Instance(rect: SIMD4(Float(x * input.scale), 0, Float(2 * input.scale), Float(input.height * input.scale)),
                                          color: SIMD4(1, 0.60, 0.28, 1)))
            }
        }
        let length = max(1, instances.count) * MemoryLayout<Instance>.stride
        if (buffers[slot]?.length ?? 0) < length { buffers[slot] = device.makeBuffer(length: length, options: .storageModeShared) }
        guard let buffer = buffers[slot], let command = queue.makeCommandBuffer() else { return }
        instances.withUnsafeBytes { bytes in
            if let source = bytes.baseAddress { buffer.contents().copyMemory(from: source, byteCount: bytes.count) }
        }
        let pass = MTLRenderPassDescriptor()
        pass.colorAttachments[0].texture = update.drawable.texture
        pass.colorAttachments[0].loadAction = .clear
        pass.colorAttachments[0].storeAction = .store
        pass.colorAttachments[0].clearColor = MTLClearColor(red: 0.09, green: 0.10, blue: 0.12, alpha: 1)
        guard let encoder = command.makeRenderCommandEncoder(descriptor: pass) else { return }
        var viewport = SIMD2(Float(update.drawable.texture.width), Float(update.drawable.texture.height))
        encoder.setRenderPipelineState(pipeline)
        encoder.setVertexBuffer(buffer, offset: 0, index: 0)
        encoder.setVertexBytes(&viewport, length: MemoryLayout<SIMD2<Float>>.stride, index: 1)
        encoder.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 6, instanceCount: instances.count)
        encoder.endEncoding()
        let slots = slots
        command.addCompletedHandler { _ in slots.release(slot) }
        command.present(update.drawable)
        command.commit()
        committed = true
        drawnRevision = revision
        state.withLock { value in
            value.measurement.frames += 1
            if !Thread.isMainThread { value.measurement.framesOffMain += 1 }
            if value.mainBusy { value.measurement.framesWhileMainBusy += 1 }
            value.measurement.maxCPUSeconds = max(value.measurement.maxCPUSeconds, CACurrentMediaTime() - started)
        }
        Task { @MainActor [onElements, onPiano] in
            if let piano { onPiano?(piano.0, input.snapshot) } else { onElements(shown) }
        }
    }
}

private let arrangeShader = """
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
"""
