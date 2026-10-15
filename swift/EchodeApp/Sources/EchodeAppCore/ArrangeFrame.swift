import EchodeBridge
import Foundation

struct ArrangeHeaderPalette: Sendable, Equatable {
    var background = SIMD4<Float>(0.12, 0.13, 0.16, 1)
    var selected = SIMD4<Float>(0.18, 0.23, 0.30, 1)
    var foreground = SIMD4<Float>(0.90, 0.91, 0.94, 1)
    var selectedForeground = SIMD4<Float>(1, 1, 1, 1)
    var separator = SIMD4<Float>(0.27, 0.29, 0.33, 1)
}

struct ArrangeHeaderInput: Sendable, Equatable {
    let width: Double
    let palette: ArrangeHeaderPalette
}

struct ArrangeHeaderRow: Sendable {
    let id: UUID
    let name: String
    let y: Float
    let height: Float
}

struct WaveformDraw: Sendable {
    var rect: SIMD4<Float>
    var color: SIMD4<Float>
    var columns: [SIMD2<Float>]
}

/// A complete arrange frame. Headers are derived from the exact lane floats
/// returned by Rust, and submitted with those lanes to one drawable.
struct ArrangeFrame: Sendable {
    let revision: UInt64
    let input: CanvasInput
    let camera: ArrangeCamera
    let timelineOrigin: Double
    let timelineWidth: Double
    let elements: [ArrangeElement]
    let headers: [ArrangeHeaderRow]
    let waveforms: [WaveformDraw]

    init(input: CanvasInput, camera: ArrangeCamera? = nil, revision: UInt64 = 0) {
        self.revision = revision
        self.input = input
        let camera = camera ?? input.camera
        self.camera = camera
        let headerWidth = min(input.width, max(0, input.header?.width ?? 0))
        timelineOrigin = input.header == nil ? 0 : min(input.width, headerWidth + 1)
        timelineWidth = max(0, input.width - timelineOrigin)
        elements = input.snapshot?.visibleElements(
            originTick: Int64((camera.originBeat * 705_600).rounded()),
            widthTicks: Int64((max(1, timelineWidth) / camera.pixelsPerBeat * 705_600).rounded(.up)),
            originTrack: camera.originTrack, heightTracks: input.height / camera.pixelsPerTrack,
            pixelsPerTick: camera.pixelsPerBeat / 705_600, pixelsPerTrack: camera.pixelsPerTrack
        ).elements ?? []
        waveforms = Self.waveforms(elements: elements, input: input, camera: camera, origin: timelineOrigin)
        guard input.header != nil, let snapshot = input.snapshot else { headers = []; return }
        let rows = camera.rows(trackCount: snapshot.trackCount, height: input.height)
        headers = zip(rows.visible, elements.filter { $0.role == .track }).compactMap { index, lane in
            guard let name = snapshot.trackName(at: index) else { return nil }
            return ArrangeHeaderRow(id: lane.id.track, name: name, y: lane.y, height: lane.height)
        }
    }

    var playheadTick: Int64? { input.playback?.position.tick }
    var playheadPlaying: Bool? { input.playback?.position.playing }
    /// Timeline x in points, including the header gutter. Nil when no playhead is drawn.
    var playheadX: Double? {
        guard let position = input.playback?.position else { return nil }
        let x = (Double(position.tick) / 705_600 - camera.originBeat) * camera.pixelsPerBeat
        guard x >= 0, x < timelineWidth else { return nil }
        return timelineOrigin + x
    }

    /// Index in `timelineInstances` where notes and the playhead begin.
    var foregroundStart: Int {
        let lanes = elements.filter { $0.role == .track }.count
        let clips = elements.filter { $0.role == .clip }.count
        let firstBeat = Int(camera.originBeat.rounded(.up))
        let lastBeat = Int((camera.originBeat + timelineWidth / camera.pixelsPerBeat).rounded(.up))
        return lanes + (max(firstBeat, lastBeat) - firstBeat + 1) + clips
    }

    var timelineInstances: [Instance] {
        let lanes = elements.filter { $0.role == .track }
        let clips = elements.filter { $0.role == .clip }
        let notes = elements.filter { $0.role == .note }
        var instances = lanes.map(instance)
        let firstBeat = Int(camera.originBeat.rounded(.up))
        let lastBeat = Int((camera.originBeat + timelineWidth / camera.pixelsPerBeat).rounded(.up))
        for beat in firstBeat...max(firstBeat, lastBeat) {
            let x = (timelineOrigin + (Double(beat) - camera.originBeat) * camera.pixelsPerBeat) * input.scale
            instances.append(Instance(rect: SIMD4(Float(x), 0, Float(input.scale), Float(input.height * input.scale)),
                                      color: beat % 4 == 0 ? SIMD4(0.27, 0.29, 0.33, 1) : SIMD4(0.18, 0.20, 0.23, 1)))
        }
        instances.append(contentsOf: clips.map(instance))
        instances.append(contentsOf: notes.map(instance))
        if let position = input.playback?.position {
            let x = (Double(position.tick) / 705_600 - camera.originBeat) * camera.pixelsPerBeat
            if x >= 0, x < timelineWidth {
                instances.append(Instance(rect: SIMD4(Float((timelineOrigin + x) * input.scale), 0, Float(2 * input.scale), Float(input.height * input.scale)),
                                          color: SIMD4(1, 0.60, 0.28, 1)))
            }
        }
        return instances
    }

    var headerInstances: [Instance] {
        guard let header = input.header else { return [] }
        let width = Float(header.width)
        let palette = header.palette
        var instances = [Instance(rect: SIMD4(0, 0, Float(timelineOrigin), Float(input.height)) * Float(input.scale), color: palette.background)]
        for row in headers {
            let chosen = row.id == input.selected
            instances.append(Instance(rect: SIMD4(0, row.y, width, max(0, row.height - 1)) * Float(input.scale),
                                      color: chosen ? palette.selected : palette.background))
            instances.append(Instance(rect: SIMD4(0, row.y + row.height - 1, width, 1) * Float(input.scale), color: palette.separator))
            for (bar, height) in [4.0, 8.0, 12.0, 8.0, 4.0].enumerated() {
                instances.append(Instance(rect: SIMD4(Float(8 + bar * 3), row.y + (row.height - Float(height)) / 2, 1.5, Float(height)) * Float(input.scale),
                                          color: chosen ? palette.selectedForeground : palette.foreground))
            }
        }
        instances.append(Instance(rect: SIMD4(width, 0, 1, Float(input.height)) * Float(input.scale), color: palette.separator))
        return instances
    }

    private func instance(_ element: ArrangeElement) -> Instance {
        let color: SIMD4<Float>
        switch element.role {
        case .track: color = element.id.track == input.selected ? SIMD4(0.18, 0.23, 0.30, 1) : SIMD4(0.12, 0.13, 0.16, 1)
        case .clip: color = SIMD4(0.18, 0.36, 0.47, 1)
        case .note: color = SIMD4(0.52, 0.83, 0.88, 1)
        }
        return Instance(rect: SIMD4(Float(timelineOrigin) + element.x, element.y, element.width, max(1, element.height - 1)) * Float(input.scale), color: color)
    }

    private static func waveforms(elements: [ArrangeElement], input: CanvasInput, camera: ArrangeCamera, origin: Double) -> [WaveformDraw] {
        guard let snapshot = input.snapshot else { return [] }
        let originTick = Int64((camera.originBeat * 705_600).rounded())
        let pixelsPerTick = camera.pixelsPerBeat / 705_600
        guard pixelsPerTick > 0 else { return [] }
        let color = SIMD4<Float>(0.78, 0.93, 0.96, 1)
        return elements.compactMap { element in
            guard element.role == .clip, let placement = element.id.placement else { return nil }
            let columns = max(1, Int((Double(element.width) * input.scale).rounded(.up)))
            let start = originTick + Int64((Double(element.x) / pixelsPerTick).rounded())
            let end = originTick + Int64((Double(element.x + element.width) / pixelsPerTick).rounded())
            let samples = snapshot.waveform(placement: placement, startTick: start, endTick: max(start + 1, end), columns: columns)
            guard !samples.isEmpty else { return nil }
            let rect = SIMD4(Float(origin) + element.x, element.y, element.width, max(1, element.height - 1)) * Float(input.scale)
            return WaveformDraw(rect: rect, color: color, columns: samples)
        }
    }
}
