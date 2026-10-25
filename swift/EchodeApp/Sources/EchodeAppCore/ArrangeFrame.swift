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
    var selectedTracks: Set<UUID>? = nil
    var collapsedTracks: Set<UUID> = []
    var pluginNames: [String: String] = [:]
}

struct ArrangeHeaderRow: Sendable {
    let id: UUID
    let name: String
    let y: Float
    let height: Float
    let mix: TrackMix?
    var peak: Float
    let depth: Int
    let folder: Bool
    let collapsed: Bool
    let devices: [TrackDevice]
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
    var headers: [ArrangeHeaderRow]
    let waveforms: [WaveformDraw]

    init(input: CanvasInput, camera: ArrangeCamera? = nil, revision: UInt64 = 0) {
        self.revision = revision
        self.input = input
        let camera = camera ?? input.camera
        self.camera = camera
        timelineOrigin = input.timelineOrigin
        timelineWidth = max(0, input.width - timelineOrigin)
        elements = input.snapshot?.visibleElements(
            originTick: Int64((camera.originBeat * 705_600).rounded()),
            widthTicks: Int64((max(1, timelineWidth) / camera.pixelsPerBeat * 705_600).rounded(.up)),
            originTrack: camera.originTrack, heightTracks: input.arrangeHeight / camera.pixelsPerTrack,
            pixelsPerTick: camera.pixelsPerBeat / 705_600, pixelsPerTrack: camera.pixelsPerTrack
        ).elements ?? []
        waveforms = Self.waveforms(elements: elements, input: input, camera: camera, origin: timelineOrigin)
        guard input.header != nil, let snapshot = input.snapshot else { headers = []; return }
        let rows = camera.rows(trackCount: snapshot.trackCount, height: input.arrangeHeight)
        headers = zip(rows.visible, elements.filter { $0.role == .track }).compactMap { index, lane in
            guard let name = snapshot.trackName(at: index) else { return nil }
            let hierarchy = snapshot.trackHierarchy(at: index)
            return ArrangeHeaderRow(id: lane.id.track, name: name, y: Float(input.arrangeTop) + lane.y, height: lane.height,
                                    mix: snapshot.mixer(onTrack: index), peak: input.playback?.peak(track: lane.id.track) ?? 0,
                                    depth: hierarchy?.depth ?? 0, folder: hierarchy?.hasChildren ?? false,
                                    collapsed: input.header?.collapsedTracks.contains(lane.id.track) == true, devices: snapshot.devices(onTrack: index))
        }
    }

    mutating func refreshMeters() {
        for index in headers.indices {
            headers[index].peak = input.playback?.peak(track: headers[index].id) ?? 0
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
            instances.append(Instance(rect: SIMD4(Float(x), Float(input.arrangeTop * input.scale), Float(input.scale), Float(input.arrangeHeight * input.scale)),
                                      color: beat % 4 == 0 ? SIMD4(0.27, 0.29, 0.33, 1) : SIMD4(0.18, 0.20, 0.23, 1)))
        }
        instances.append(contentsOf: clips.map(instance))
        instances.append(contentsOf: notes.map(instance))
        instances.append(contentsOf: clipControlInstances)
        instances.append(contentsOf: clipMoveInstances)
        if let position = input.playback?.position {
            let x = (Double(position.tick) / 705_600 - camera.originBeat) * camera.pixelsPerBeat
            if x >= 0, x < timelineWidth {
                instances.append(Instance(rect: SIMD4(Float((timelineOrigin + x) * input.scale), 0, Float(2 * input.scale), Float(input.height * input.scale)),
                                          color: SIMD4(1, 0.60, 0.28, 1)))
            }
        }
        return instances
    }

    func isTrackSelected(_ id: UUID) -> Bool {
        input.header?.selectedTracks?.contains(id) ?? (id == input.selected)
    }

    var headerInstances: [Instance] {
        guard let header = input.header else { return rulerInstances }
        let width = Float(header.width)
        let palette = header.palette
        var instances = [Instance(rect: SIMD4(0, 0, Float(timelineOrigin), Float(input.height)) * Float(input.scale), color: palette.background)]
        for row in headers {
            let chosen = isTrackSelected(row.id)
            instances.append(Instance(rect: SIMD4(0, row.y, width, max(0, row.height - 1)) * Float(input.scale),
                                      color: chosen ? palette.selected : palette.background))
            instances.append(Instance(rect: SIMD4(0, row.y + row.height - 1, width, 1) * Float(input.scale), color: palette.separator))
            let fx = TrackFXLayout(row: CGRect(x: 0, y: Double(row.y), width: Double(width), height: Double(row.height)))
            if fx.visible {
                instances.append(Instance(rect: SIMD4(Float(fx.rect.minX), Float(fx.rect.minY), Float(fx.rect.width), Float(fx.rect.height)) * Float(input.scale), color: palette.separator))
            }
            let names = TrackFXNamesLayout(row: CGRect(x: 0, y: Double(row.y), width: Double(width), height: Double(row.height)), count: row.devices.count, depth: row.depth)
            for rect in [names.name, names.overflow] where !rect.isEmpty {
                instances.append(Instance(rect: SIMD4(Float(rect.minX), Float(rect.minY), Float(rect.width), Float(rect.height)) * Float(input.scale), color: palette.separator))
            }
            let layout = TrackControlLayout(row: CGRect(x: 0, y: Double(row.y), width: Double(width), height: Double(row.height)))
            if layout.visible, let mix = row.mix {
                func add(_ rect: CGRect, _ color: SIMD4<Float>) {
                    instances.append(Instance(rect: SIMD4(Float(rect.minX), Float(rect.minY), Float(rect.width), Float(rect.height)) * Float(input.scale), color: color))
                }
                add(layout.mute, mix.mute.value != 0 ? SIMD4(0.75, 0.25, 0.20, 1) : palette.separator)
                add(layout.solo, mix.solo.value != 0 ? SIMD4(0.70, 0.55, 0.13, 1) : palette.separator)
                let preview = input.faderPreview.flatMap { $0.track == row.id ? $0 : nil }
                for (rect, value) in [(layout.gain, MixScale.position(preview?.gain ?? mix.gain.value)),
                                      (layout.pan, Double((preview?.pan ?? mix.pan.value) + 1) / 2)] {
                    add(CGRect(x: rect.minX, y: rect.midY - 1.5, width: rect.width, height: 3), palette.separator)
                    add(CGRect(x: rect.minX + value * rect.width - 2, y: rect.minY + 3, width: 4, height: rect.height - 6), palette.foreground)
                }
                add(layout.meter, palette.separator)
                add(CGRect(x: layout.meter.minX, y: layout.meter.minY, width: layout.meter.width * MixScale.meter(row.peak), height: layout.meter.height),
                    row.peak >= 1 ? SIMD4(1, 0.25, 0.12, 1) : SIMD4(0.28, 0.80, 0.49, 1))
                continue
            }
            let folder = TrackFolderLayout(row: CGRect(x: 0, y: Double(row.y), width: Double(width), height: Double(row.height)), depth: row.depth, folder: row.folder)
            for (bar, height) in (row.folder ? [] : [4.0, 8.0, 12.0, 8.0, 4.0]).enumerated() {
                instances.append(Instance(rect: SIMD4(Float(folder.titleX - 22) + Float(bar * 3), row.y + (row.height - Float(height)) / 2, 1.5, Float(height)) * Float(input.scale),
                                          color: chosen ? palette.selectedForeground : palette.foreground))
            }
        }
        instances.append(Instance(rect: SIMD4(width, 0, 1, Float(input.height)) * Float(input.scale), color: palette.separator))
        return instances + rulerInstances
    }

    var clipControlLayout: ClipControlLayout? {
        guard let id = input.selectedClip, let clip = input.snapshot?.clipMix(id), clip.isAudio,
              let element = elements.first(where: { $0.role == .clip && $0.id.placement == id }) else { return nil }
        let draft = input.clipDraft.flatMap { $0.original.id == id ? $0 : nil }
        return ClipControlLayout.make(clip, camera: camera, origin: timelineOrigin,
            y: input.arrangeTop + Double(element.y), height: Double(element.height), fadeInTicks: draft?.fadeInTicks, fadeOutTicks: draft?.fadeOutTicks)
    }

    private var clipControlInstances: [Instance] {
        guard let layout = clipControlLayout else { return [] }
        let color = SIMD4<Float>(0.93, 0.96, 0.85, 1)
        func instance(_ rect: CGRect) -> Instance {
            Instance(rect: SIMD4(Float(rect.minX), Float(rect.minY), Float(rect.width), Float(rect.height)) * Float(input.scale), color: color)
        }
        var result = [instance(layout.entering), instance(layout.leaving), instance(layout.gain)]
        let left = max(timelineOrigin, layout.rect.minX)
        let right = min(input.width, layout.rect.maxX)
        guard right > left else { return result }
        // At most one line sample per visible point, independent of clip duration.
        for offset in 0..<Int((right - left).rounded(.up)) {
            let x = left + Double(offset)
            let entering = layout.fadeIn > 0 ? (x - layout.rect.minX) / layout.fadeIn : 1
            let leaving = layout.fadeOut > 0 ? (layout.rect.maxX - x) / layout.fadeOut : 1
            let gain = max(0, min(1, entering, leaving))
            result.append(instance(CGRect(x: x, y: layout.rect.minY + (1 - gain) * (layout.rect.height - 1), width: 1, height: 1)))
        }
        return result
    }

    private func instance(_ element: ArrangeElement) -> Instance {
        let color: SIMD4<Float>
        switch element.role {
        case .track: color = isTrackSelected(element.id.track) ? SIMD4(0.18, 0.23, 0.30, 1) : SIMD4(0.12, 0.13, 0.16, 1)
        case .clip: color = element.id.placement == input.selectedClip ? SIMD4(0.28, 0.54, 0.66, 1) : SIMD4(0.18, 0.36, 0.47, 1)
        case .note: color = SIMD4(0.52, 0.83, 0.88, 1)
        }
        return Instance(rect: SIMD4(Float(timelineOrigin) + element.x, Float(input.arrangeTop) + element.y, element.width, max(1, element.height - 1)) * Float(input.scale), color: color)
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
            let rect = SIMD4(Float(origin) + element.x, Float(input.arrangeTop) + element.y, element.width, max(1, element.height - 1)) * Float(input.scale)
            return WaveformDraw(rect: rect, color: color, columns: samples)
        }
    }
}
