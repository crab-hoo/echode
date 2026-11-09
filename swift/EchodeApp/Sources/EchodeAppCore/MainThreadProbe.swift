import AppKit
import EchodeBridge
import Foundation
import Metal
import SwiftUI
import Synchronization

/// Milliseconds of one probe scenario.
private struct ProbeTiming: Codable {
    let samples: Int
    let p50: Double
    let p95: Double
    let max: Double
    /// The sample that took `max`: 0 is the first event of the scenario.
    let maxAt: Int

    init(_ values: [Double]) {
        maxAt = values.indices.max { values[$0] < values[$1] } ?? 0
        let sorted = values.sorted()
        samples = sorted.count
        p50 = sorted.isEmpty ? 0 : sorted[sorted.count / 2]
        p95 = sorted.isEmpty ? 0 : sorted[min(sorted.count - 1, Int((Double(sorted.count) * 0.95).rounded(.down)))]
        max = sorted.last ?? 0
    }
}

private struct MainThreadReport: Codable {
    let tracks: Int
    let notesPerTrack: Int
    let windowPoints: [Double]
    /// One playback poll that moves only the position, through layout.
    let positionPoll: ProbeTiming
    /// A poll whose state is unchanged, through layout.
    let unchangedPoll: ProbeTiming
    /// A reply that changes only the sample audition, through layout.
    let auditionReply: ProbeTiming
    /// A reply that changes the status (here the metronome), through layout.
    let statusReply: ProbeTiming
    /// One drag event of each divider, through layout.
    let divider: [String: ProbeTiming]
    /// CPU time of one arrange frame's encode, labels included.
    let arrangeFrame: [String: ProbeTiming]
    /// Labels the arrange rasterized per frame, by scenario.
    let arrangeRasterized: [String: Double]
}

/// Explicit probe for the main-thread load of audit item E2 (FCA-17, JNK-05).
/// No window, display link, audio device or plugin helper is opened. Project
/// size, window size and event counts are scoped to this measurement only.
@MainActor
public func runMainThreadProbe(reportPath: String) async throws {
    let trackCount = 128, notesPerTrack = 32
    let positions = Mutex<Int64>(0)
    let controller = ProjectController(sendAudio: { _, _ in
        try probePlayingState(tick: positions.withLock { $0 &+= 240; return $0 })
    })
    // The workspace holds a silent audition output while visible; this probe opens no device.
    controller.sendAuditionSettings = { _, _, _ in }
    let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-main-thread-\(UUID()).echode")
    defer { try? FileManager.default.removeItem(at: url) }
    await controller.open(url, creating: true)
    guard let document = controller.documentID else { throw CocoaError(.coderValueNotFound) }
    let tracks = (0..<trackCount).map { EditAction.addTrack(name: "Track \($0)") }
    guard await controller.performActions({ tracks }, documentID: document) else { throw CocoaError(.coderInvalidValue) }
    let ids = controller.tracks.map(\.id)
    let notes = ids.flatMap { id in
        (0..<notesPerTrack).map { index in
            EditAction.insertNote(track: id.uuidString, pitch: UInt8(48 + index % 24), velocity: 100,
                                  start: Int64(index) * 960, length: 480)
        }
    }
    guard await controller.performActions({ notes }, documentID: document), let first = ids.first else { throw CocoaError(.coderInvalidValue) }
    controller.selectTrack(first)

    let suite = "org.echode.probe.main-thread.\(UUID().uuidString)"
    guard let defaults = UserDefaults(suiteName: suite) else { throw CocoaError(.featureUnsupported) }
    defer { defaults.removePersistentDomain(forName: suite) }
    let windowSize = CGSize(width: 1440, height: 900)
    let host = NSHostingView(rootView: WorkspaceView(controller: controller, state: WorkspaceState(defaults: defaults)))
    host.frame = CGRect(origin: .zero, size: windowSize)
    host.layoutSubtreeIfNeeded()

    let playing = try (1...400).map { try probePlayingState(tick: Int64($0) * 240) }
    func measured(_ count: Int, _ change: (Int) -> Void) -> [Double] {
        (0..<count).map { index in
            let start = ContinuousClock.now
            change(index)
            host.layoutSubtreeIfNeeded()
            return milliseconds(ContinuousClock.now - start)
        }
    }
    // Warm the hierarchy once with a playing state before measuring.
    _ = measured(20) { controller.session.receive(playing[$0]) }
    // For `sample`: polls repeat this long after "profiling" appears on stderr.
    if let seconds = ProcessInfo.processInfo.environment["ECHODE_MAIN_THREAD_PROFILE_SECONDS"].flatMap(Double.init) {
        FileHandle.standardError.write(Data("profiling\n".utf8))
        let end = ContinuousClock.now + .seconds(seconds)
        var index = 0
        while ContinuousClock.now < end { controller.session.receive(playing[index % 400]); host.layoutSubtreeIfNeeded(); index += 1 }
    }
    let position = measured(200) { controller.session.receive(playing[20 + $0]) }
    let held = playing[220]
    controller.session.receive(held)
    host.layoutSubtreeIfNeeded()
    let unchanged = measured(200) { _ in controller.session.receive(held) }
    let auditions = try (1...100).map { try probePlayingState(tick: 220 * 240, audition: UInt64($0)) }
    let audition = measured(auditions.count) { controller.session.receive(auditions[$0]) }
    let statuses = try (1...100).map { try probePlayingState(tick: 220 * 240, metronome: $0 % 2 == 0) }
    let status = measured(statuses.count) { controller.session.receive(statuses[$0]) }

    var divider: [String: ProbeTiming] = [:]
    for splitter in probeSplitters(in: host) {
        let name = switch splitter.pane { case .inspector: "inspector"; case .browser: "browser"; case .editor: "editor"; default: "other" }
        guard let down = probeMouse(.leftMouseDown, at: .zero) else { continue }
        splitter.mouseDown(with: down)
        let events = (0..<120).compactMap { index in
            let offset = Double(index < 60 ? index : 120 - index)
            return probeMouse(.leftMouseDragged, at: splitter.pane == .editor ? CGPoint(x: 0, y: offset) : CGPoint(x: offset, y: 0))
        }
        divider[name] = ProbeTiming(measured(events.count) { splitter.mouseDragged(with: events[$0]) })
        if let up = probeMouse(.leftMouseUp, at: .zero) { splitter.mouseUp(with: up) }
        host.layoutSubtreeIfNeeded()
    }

    let (frames, rasterized) = try probeArrangeFrames(snapshot: controller.snapshot, selected: first)
    await controller.close()
    let report = MainThreadReport(tracks: trackCount, notesPerTrack: notesPerTrack, windowPoints: [windowSize.width, windowSize.height],
                                  positionPoll: ProbeTiming(position), unchangedPoll: ProbeTiming(unchanged),
                                  auditionReply: ProbeTiming(audition), statusReply: ProbeTiming(status),
                                  divider: divider, arrangeFrame: frames, arrangeRasterized: rasterized)
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
    try encoder.encode(report).write(to: URL(fileURLWithPath: reportPath))
}

/// Encodes arrange frames as the render thread does, without waiting for a
/// display. GPU completion waits belong to this probe only.
@MainActor
private func probeArrangeFrames(snapshot: Snapshot?, selected: UUID) throws -> ([String: ProbeTiming], [String: Double]) {
    guard let snapshot, let device = MTLCreateSystemDefaultDevice(), let queue = device.makeCommandQueue() else { throw CocoaError(.featureUnsupported) }
    let painter = try ArrangePainter(device: device)
    func input(pixelsPerBeat: Double) -> CanvasInput {
        CanvasInput(snapshot: snapshot, selected: selected,
                    camera: ArrangeCamera(originBeat: 0, originTrack: 0, pixelsPerBeat: pixelsPerBeat, pixelsPerTrack: 40),
                    width: 1000, height: 400, scale: 2,
                    header: ArrangeHeaderInput(width: 180, palette: ArrangeHeaderPalette()),
                    arrangeRuler: ArrangeRulerInput(start: nil, loop: nil))
    }
    var timings: [String: ProbeTiming] = [:]
    var rasterized: [String: Double] = [:]
    let scenarios: [(String, [Double])] = [
        // Playback redraws the same view every frame.
        ("steady", Array(repeating: 24, count: 120)),
        // A pinch from 6 to 52 points per beat changes every ruler label width.
        ("zoom", (0..<100).map { 6 * pow(52.0 / 6, Double($0) / 99) }),
    ]
    for (name, zooms) in scenarios {
        _ = try encodeProbeFrame(ArrangeFrame(input: input(pixelsPerBeat: zooms[0])), painter: painter, device: device, queue: queue)
        let before = painter.rasterizedLabels
        timings[name] = ProbeTiming(try zooms.map { try encodeProbeFrame(ArrangeFrame(input: input(pixelsPerBeat: $0)), painter: painter, device: device, queue: queue) })
        rasterized[name] = Double(painter.rasterizedLabels - before) / Double(zooms.count)
    }
    return (timings, rasterized)
}

/// Returns the CPU milliseconds of the encode alone.
private func encodeProbeFrame(_ frame: ArrangeFrame, painter: ArrangePainter, device: any MTLDevice, queue: any MTLCommandQueue) throws -> Double {
    let size = frame.input.drawableSize
    let descriptor = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .bgra8Unorm, width: max(1, Int(size.width)), height: max(1, Int(size.height)), mipmapped: false)
    descriptor.storageMode = .private
    descriptor.usage = .renderTarget
    let instances = frame.timelineInstances + frame.headerInstances
    guard let texture = device.makeTexture(descriptor: descriptor), let command = queue.makeCommandBuffer(),
          let buffer = device.makeBuffer(length: max(1, instances.count) * MemoryLayout<Instance>.stride, options: .storageModeShared) else {
        throw CocoaError(.featureUnsupported)
    }
    instances.withUnsafeBytes { data in
        if let source = data.baseAddress { buffer.contents().copyMemory(from: source, byteCount: data.count) }
    }
    let pass = MTLRenderPassDescriptor()
    pass.colorAttachments[0].texture = texture
    pass.colorAttachments[0].loadAction = .clear
    pass.colorAttachments[0].storeAction = .store
    guard let encoder = command.makeRenderCommandEncoder(descriptor: pass) else { throw CocoaError(.featureUnsupported) }
    let start = ContinuousClock.now
    painter.encode(encoder: encoder, texture: texture, buffer: buffer, timelineCount: frame.timelineInstances.count,
                   headerCount: frame.headerInstances.count, frame: frame)
    let elapsed = milliseconds(ContinuousClock.now - start)
    encoder.endEncoding()
    command.commit()
    command.waitUntilCompleted()
    if let error = command.error { throw error }
    return elapsed
}

private func probePlayingState(tick: Int64, audition: UInt64? = nil, metronome: Bool = false) throws -> AudioState {
    let sample = audition.map { #","audition":{"request":\#($0),"handled":\#($0),"phase":"sounding","history":[]}"# } ?? ""
    let json = #"{"playing":true,"running":true,"opening":false,"tick":\#(tick),"devices":[],"metronome":\#(metronome)\#(sample)}"#
    return try JSONDecoder().decode(AudioState.self, from: Data(json.utf8))
}

@MainActor
private func probeSplitters(in view: NSView) -> [WorkspaceSplitter] {
    (view as? WorkspaceSplitter).map { [$0] } ?? view.subviews.flatMap(probeSplitters)
}

private func probeMouse(_ type: NSEvent.EventType, at point: CGPoint) -> NSEvent? {
    NSEvent.mouseEvent(with: type, location: point, modifierFlags: [], timestamp: 0, windowNumber: 0,
                       context: nil, eventNumber: 0, clickCount: 1, pressure: 1)
}

private func milliseconds(_ duration: Duration) -> Double {
    Double(duration.components.seconds) * 1_000 + Double(duration.components.attoseconds) / 1e15
}
