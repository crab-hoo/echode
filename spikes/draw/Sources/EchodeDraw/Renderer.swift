import AppKit
import CEchodeDraw
import Metal
import QuartzCore

struct Camera {
    var originX: Float
    var originY: Float
    var zoomX: Float
    var zoomY: Float
}

@MainActor
final class LayerHost: NSView {
    let metalLayer = CAMetalLayer()

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not used")
    }

    override func makeBackingLayer() -> CALayer {
        metalLayer
    }
}

@MainActor
final class Renderer: NSObject, CAMetalDisplayLinkDelegate {
    let hostView: LayerHost
    private let metalLayer: CAMetalLayer
    private let device: MTLDevice
    private let queue: MTLCommandQueue
    private let pipeline: MTLRenderPipelineState
    private let scene: OpaquePointer
    private let trackBuffer: MTLBuffer
    private let noteBuffer: MTLBuffer
    private let trackCapacity: Int
    private let noteCapacity: Int
    private let bench: Bool
    private let reportURL: URL?
    private var link: CAMetalDisplayLink?
    private var frame = 0
    private var samples: [Double] = []
    private var visibleNotes: [Int] = []
    private var missed = 0
    private var previousTarget: CFTimeInterval?
    private var preferredFPS: Double = 60
    private var touchSpan: (x: CGFloat, y: CGFloat)?
    private var camera = Camera(originX: 0, originY: 0, zoomX: 72, zoomY: 26)

    init?(bench: Bool, reportURL: URL?) {
        guard let device = MTLCreateSystemDefaultDevice(),
            let queue = device.makeCommandQueue()
        else {
            return nil
        }
        guard let scene = echode_scene_new() else { return nil }
        let trackCapacity = Int(echode_scene_track_count(scene))
        let noteCapacity = Int(echode_scene_note_count(scene))
        guard
            let trackBuffer = device.makeBuffer(
                length: MemoryLayout<EchodeInstance>.stride * trackCapacity,
                options: .storageModeShared
            ),
            let noteBuffer = device.makeBuffer(
                length: MemoryLayout<EchodeInstance>.stride * noteCapacity,
                options: .storageModeShared
            )
        else {
            echode_scene_free(scene)
            return nil
        }
        let library: MTLLibrary
        do {
            library = try device.makeLibrary(source: noteShader, options: nil)
        } catch {
            echode_scene_free(scene)
            return nil
        }
        guard let vertex = library.makeFunction(name: "note_vertex"),
            let fragment = library.makeFunction(name: "note_fragment")
        else {
            echode_scene_free(scene)
            return nil
        }
        let descriptor = MTLRenderPipelineDescriptor()
        descriptor.vertexFunction = vertex
        descriptor.fragmentFunction = fragment
        descriptor.colorAttachments[0].pixelFormat = .bgra8Unorm
        guard let pipeline = try? device.makeRenderPipelineState(descriptor: descriptor) else {
            echode_scene_free(scene)
            return nil
        }

        let hostView = LayerHost(frame: .zero)
        hostView.metalLayer.device = device
        hostView.metalLayer.pixelFormat = .bgra8Unorm
        hostView.metalLayer.framebufferOnly = true
        hostView.metalLayer.displaySyncEnabled = true
        hostView.metalLayer.maximumDrawableCount = 3
        self.hostView = hostView
        self.metalLayer = hostView.metalLayer
        self.device = device
        self.queue = queue
        self.pipeline = pipeline
        self.scene = scene
        self.trackBuffer = trackBuffer
        self.noteBuffer = noteBuffer
        self.trackCapacity = trackCapacity
        self.noteCapacity = noteCapacity
        self.bench = bench
        self.reportURL = reportURL

        super.init()
    }

    func start() {
        let maximum = Double(hostView.window?.screen?.maximumFramesPerSecond ?? 60)
        preferredFPS = maximum
        let link = CAMetalDisplayLink(metalLayer: metalLayer)
        link.delegate = self
        link.preferredFrameRateRange = CAFrameRateRange(
            minimum: Float(min(60, maximum)),
            maximum: Float(maximum),
            preferred: Float(maximum)
        )
        link.add(to: .main, forMode: .common)
        self.link = link
    }

    func scroll(by delta: CGSize) {
        guard !bench else { return }
        camera.originX -= Float(delta.width) / camera.zoomX
        camera.originY -= Float(delta.height) / camera.zoomY
    }

    func magnify(by magnification: CGFloat) {
        guard !bench else { return }
        let factor = Float(max(0.05, 1 + magnification))
        camera.zoomX = min(4_000, max(0.25, camera.zoomX * factor))
        camera.zoomY = min(400, max(0.25, camera.zoomY * factor))
    }

    func beginTouches(_ event: NSEvent) {
        touchSpan = span(of: event)
    }

    func moveTouches(_ event: NSEvent) {
        guard !bench, let previous = touchSpan, let current = span(of: event) else { return }
        if previous.x > 2 {
            camera.zoomX = min(4_000, max(0.25, camera.zoomX * Float(current.x / previous.x)))
        }
        if previous.y > 2 {
            camera.zoomY = min(400, max(0.25, camera.zoomY * Float(current.y / previous.y)))
        }
        touchSpan = current
    }

    func endTouches() {
        touchSpan = nil
    }

    nonisolated func metalDisplayLink(
        _ link: CAMetalDisplayLink,
        needsUpdate update: CAMetalDisplayLink.Update
    ) {
        let drawable = DrawableBox(update.drawable)
        let target = update.targetTimestamp
        MainActor.assumeIsolated {
            self.render(drawable: drawable.drawable, target: target)
        }
    }

    private func render(drawable: any CAMetalDrawable, target: CFTimeInterval) {
        let scale = hostView.window?.backingScaleFactor ?? NSScreen.main?.backingScaleFactor ?? 2
        metalLayer.contentsScale = scale
        metalLayer.drawableSize = CGSize(
            width: max(hostView.bounds.width, 1) * scale,
            height: max(hostView.bounds.height, 1) * scale
        )
        let viewport = SIMD2<Float>(
            Float(drawable.texture.width),
            Float(drawable.texture.height)
        )
        guard viewport.x >= 1, viewport.y >= 1 else { return }

        let started = CACurrentMediaTime()
        let camera = bench ? benchmarkCamera(frame: frame, viewport: viewport) : self.camera
        var droppedTracks: UInt32 = 0
        var droppedNotes: UInt32 = 0
        let trackCount = echode_scene_visible_tracks(
            scene,
            camera.originX,
            camera.originY,
            viewport.x,
            viewport.y,
            camera.zoomX,
            camera.zoomY,
            trackBuffer.contents().assumingMemoryBound(to: EchodeInstance.self),
            UInt32(trackCapacity),
            &droppedTracks
        )
        let noteCount = echode_scene_visible_notes(
            scene,
            camera.originX,
            camera.originY,
            viewport.x,
            viewport.y,
            camera.zoomX,
            camera.zoomY,
            noteBuffer.contents().assumingMemoryBound(to: EchodeInstance.self),
            UInt32(noteCapacity),
            &droppedNotes
        )
        encode(drawable: drawable, viewport: viewport, trackCount: trackCount, noteCount: noteCount)
        let cpu = CACurrentMediaTime() - started

        if frame >= 20 {
            samples.append(cpu)
            visibleNotes.append(Int(noteCount))
            if let previousTarget {
                let interval = target - previousTarget
                if interval > (1.5 / preferredFPS) {
                    missed += 1
                }
            }
        }
        previousTarget = target
        frame += 1

        if frame % 30 == 0 {
            let shown = Int(noteCount)
            hostView.window?.title =
                "Echode draw  \(shown) notes  \(String(format: "%.2f", cpu * 1_000)) ms"
        }
        if bench && frame >= 20 + 540 {
            finish(dropped: droppedTracks + droppedNotes)
        }
        if droppedTracks + droppedNotes > 0 && bench {
            finish(dropped: droppedTracks + droppedNotes)
        }
    }

    private func encode(
        drawable: CAMetalDrawable,
        viewport: SIMD2<Float>,
        trackCount: UInt32,
        noteCount: UInt32
    ) {
        guard let commandBuffer = queue.makeCommandBuffer() else { return }
        let pass = MTLRenderPassDescriptor()
        pass.colorAttachments[0].texture = drawable.texture
        pass.colorAttachments[0].loadAction = .clear
        pass.colorAttachments[0].storeAction = .store
        pass.colorAttachments[0].clearColor = MTLClearColor(red: 0.11, green: 0.11, blue: 0.12, alpha: 1)
        guard let encoder = commandBuffer.makeRenderCommandEncoder(descriptor: pass) else { return }
        encoder.setRenderPipelineState(pipeline)
        var size = viewport
        if trackCount > 0 {
            encoder.setVertexBuffer(trackBuffer, offset: 0, index: 0)
            encoder.setVertexBytes(&size, length: MemoryLayout<SIMD2<Float>>.stride, index: 1)
            encoder.drawPrimitives(
                type: .triangle,
                vertexStart: 0,
                vertexCount: 6,
                instanceCount: Int(trackCount)
            )
        }
        if noteCount > 0 {
            encoder.setVertexBuffer(noteBuffer, offset: 0, index: 0)
            encoder.setVertexBytes(&size, length: MemoryLayout<SIMD2<Float>>.stride, index: 1)
            encoder.drawPrimitives(
                type: .triangle,
                vertexStart: 0,
                vertexCount: 6,
                instanceCount: Int(noteCount)
            )
        }
        encoder.endEncoding()
        commandBuffer.present(drawable)
        commandBuffer.commit()
    }

    private func benchmarkCamera(frame: Int, viewport: SIMD2<Float>) -> Camera {
        let beats = echode_scene_timeline_beats(scene)
        let tracks = Float(echode_scene_track_count(scene))
        let fitX = viewport.x / beats * 0.9
        let fitY = viewport.y / tracks * 0.9
        switch frame {
        case ..<200:
            return Camera(
                originX: Float(frame) * 0.4,
                originY: 8,
                zoomX: 80,
                zoomY: 28
            )
        case ..<380:
            let t = Float(frame - 200) / 180
            return Camera(
                originX: (1 - t) * 30,
                originY: (1 - t) * 8,
                zoomX: lerp(80, fitX, t),
                zoomY: lerp(28, fitY, t)
            )
        default:
            let t = Float(frame - 380) / 180
            return Camera(
                originX: Float(frame - 380) * 0.55,
                originY: 30 * t,
                zoomX: lerp(fitX, 96, t),
                zoomY: lerp(fitY, 34, t)
            )
        }
    }

    private func finish(dropped: UInt32) {
        link?.invalidate()
        let budget = 1 / preferredFPS
        let sorted = samples.sorted()
        let p50 = percentile(sorted, 0.50)
        let p99 = percentile(sorted, 0.99)
        let maximum = sorted.last ?? 0
        let peakNotes = visibleNotes.max() ?? 0
        let passed = dropped == 0 && missed == 0 && p99 < budget && peakNotes == noteCapacity
        let report = """
            refresh: \(Int(preferredFPS)) Hz (budget \(format(budget)) ms)
            frames: \(samples.count) measured, \(missed) intervals over 1.5 frames
            cpu min \(format(sorted.first ?? 0))  p50 \(format(p50))  p99 \(format(p99))  max \(format(maximum)) ms
            visible notes peak: \(peakNotes) / \(noteCapacity)
            dropped instances: \(dropped)
            result: \(passed ? "pass" : "fail")

            """
        print(report)
        if let reportURL {
            try? report.write(to: reportURL, atomically: true, encoding: .utf8)
        }
        exit(passed ? 0 : 1)
    }

    private func span(of event: NSEvent) -> (x: CGFloat, y: CGFloat)? {
        let touches = Array(event.touches(matching: .touching, in: hostView))
        guard touches.count >= 2 else { return nil }
        let a = touches[0].location(in: hostView)
        let b = touches[1].location(in: hostView)
        return (abs(a.x - b.x), abs(a.y - b.y))
    }
}

private final class DrawableBox: @unchecked Sendable {
    let drawable: any CAMetalDrawable
    init(_ drawable: any CAMetalDrawable) { self.drawable = drawable }
}

private func lerp(_ from: Float, _ to: Float, _ t: Float) -> Float {
    from + (to - from) * t
}

private func percentile(_ sorted: [Double], _ p: Double) -> Double {
    guard !sorted.isEmpty else { return 0 }
    let index = Int((Double(sorted.count - 1) * p).rounded())
    return sorted[min(index, sorted.count - 1)]
}

private func format(_ seconds: Double) -> String {
    String(format: "%.3f", seconds * 1_000)
}
