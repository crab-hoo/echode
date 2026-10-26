import EchodeBridge
import Foundation

enum PianoPalette {
    static let whiteRow = SIMD4<Float>(0.14, 0.15, 0.17, 1)
    static let blackRow = SIMD4<Float>(0.09, 0.10, 0.12, 1)
    static let separator = SIMD4<Float>(0.27, 0.29, 0.33, 1)
}

struct PianoRow: Sendable {
    let pitch: Int
    let y: Float
    let height: Float
    var black: Bool { [1, 3, 6, 8, 10].contains(pitch % 12) }
}

/// Keyboard, degrees, ruler and notes are presented as one Metal frame.
struct PianoFrame: Sendable {
    let revision: UInt64
    let input: CanvasInput
    let piano: PianoRenderInput
    let layout: PianoLayout
    let notes: [PianoNote]
    let rows: [PianoRow]

    init(input: CanvasInput, piano: PianoRenderInput, revision: UInt64 = 0) {
        self.revision = revision
        self.input = input
        self.piano = piano
        let layout = PianoLayout(width: input.width, height: input.height)
        self.layout = layout
        let camera = piano.camera
        notes = input.snapshot?.pianoNotes(track: piano.track,
            originTick: PianoCamera.ticks(camera.originBeat) ?? 0,
            widthTicks: max(1, PianoCamera.ticks(layout.grid.width / camera.pixelsPerBeat) ?? 1),
            topPitch: camera.topPitch, heightPitches: layout.grid.height / camera.pixelsPerPitch,
            pixelsPerTick: camera.pixelsPerBeat / 705_600, pixelsPerPitch: camera.pixelsPerPitch) ?? []
        // MIDI pitch bounds are part of the note protocol, not a project limit.
        rows = (0...127).map { pitch in
            PianoRow(pitch: pitch, y: Float(layout.rulerHeight + (camera.topPitch - Double(pitch)) * camera.pixelsPerPitch),
                     height: Float(camera.pixelsPerPitch))
        }.filter { $0.y + $0.height > Float(layout.rulerHeight) && $0.y < Float(input.height) }
    }

    var gridInstances: [Instance] {
        let camera = piano.camera
        let origin = layout.gridOrigin
        var instances = rows.map { row in
            instance(SIMD4(Float(origin.x), row.y, Float(layout.grid.width), max(0, row.height - 1)),
                     color: row.black ? PianoPalette.blackRow : PianoPalette.whiteRow)
        }
        for beat in beats {
            let x = origin.x + (Double(beat) - camera.originBeat) * camera.pixelsPerBeat
            instances.append(instance(SIMD4(Float(x), Float(origin.y), 1, Float(layout.grid.height)),
                                      color: beat % 4 == 0 ? SIMD4(0.27, 0.29, 0.33, 1) : SIMD4(0.18, 0.20, 0.23, 1)))
        }
        for note in notes {
            instances.append(instance(SIMD4(Float(origin.x) + note.x, Float(origin.y) + note.y + 1,
                                            max(2, note.width), max(2, note.height - 2)),
                                      color: note.id == piano.focus ? SIMD4(0.99, 0.73, 0.31, 1) : SIMD4(0.42, 0.76, 0.85, 1)))
        }
        for g in [piano.preview?.geometry, piano.insertion].compactMap({ $0 }) {
            let rect = SIMD4(Float(origin.x + (Double(g.start) / 705_600 - camera.originBeat) * camera.pixelsPerBeat),
                             Float(origin.y + (camera.topPitch - Double(g.pitch)) * camera.pixelsPerPitch + 1),
                             Float(Double(g.length) / 705_600 * camera.pixelsPerBeat), Float(camera.pixelsPerPitch - 2))
            instances.append(instance(rect, color: SIMD4(0.96, 0.87, 0.54, 1)))
        }
        if let x = playheadX {
            instances.append(instance(SIMD4(Float(x), Float(origin.y), 2, Float(layout.grid.height)),
                                      color: SIMD4(1, 0.60, 0.28, 1)))
        }
        return instances
    }

    var playheadTick: Int64? { input.playback?.position.tick }
    var playheadPlaying: Bool? { input.playback?.position.playing }
    /// Playhead x in points. Nil when it falls outside the grid.
    var playheadX: Double? {
        guard let position = input.playback?.position else { return nil }
        let x = (Double(position.tick) / 705_600 - piano.camera.originBeat) * piano.camera.pixelsPerBeat
        guard x >= 0, x < layout.grid.width else { return nil }
        return layout.gridOrigin.x + x
    }

    var degreeInstances: [Instance] {
        rows.flatMap { row in
            [instance(SIMD4(0, row.y, Float(layout.degreeWidth), max(0, row.height - 1)),
                      color: piano.key.degree(row.pitch) == nil ? PianoPalette.blackRow : PianoPalette.whiteRow),
             instance(SIMD4(0, row.y + row.height - 1, Float(layout.degreeWidth), 1), color: PianoPalette.separator)]
        }
    }

    var keyboardInstances: [Instance] {
        let left = Float(layout.degreeWidth)
        let width = Float(layout.keyboardWidth)
        var instances = [instance(SIMD4(left, Float(layout.rulerHeight), width, Float(layout.grid.height)), color: SIMD4(0.25, 0.26, 0.28, 1))]
        // White keys extend behind the short black keys. Include neighbours
        // just beyond the visible rows so partial keys remain filled.
        for pitch in 0...127 where !isBlack(pitch) {
            let height = piano.camera.pixelsPerPitch
            let above = pitch < 127 && isBlack(pitch + 1) ? height / 2 : 0
            let below = pitch > 0 && isBlack(pitch - 1) ? height / 2 : 0
            let y = layout.rulerHeight + (piano.camera.topPitch - Double(pitch)) * height - above
            guard y + height + above + below > layout.rulerHeight, y < input.height else { continue }
            instances.append(instance(SIMD4(left + 1, Float(y), max(0, width - 2), Float(height + above + below - 1)),
                                      color: SIMD4(0.88, 0.89, 0.91, 1)))
        }
        for row in rows where row.black {
            instances.append(instance(SIMD4(left + width * 0.38, row.y + 1, width * 0.62, max(0, row.height - 2)),
                                      color: SIMD4(0.10, 0.11, 0.13, 1)))
        }
        return instances
    }

    var rulerInstances: [Instance] {
        var instances = [instance(SIMD4(0, 0, Float(input.width), Float(layout.rulerHeight)), color: SIMD4(0.12, 0.13, 0.16, 1)),
                         instance(SIMD4(0, Float(layout.rulerHeight - 1), Float(input.width), 1), color: SIMD4(0.27, 0.29, 0.33, 1))]
        if let rect = loopRect {
            let enabled = piano.loop?.enabled == true
            instances.append(instance(SIMD4(Float(rect.minX), Float(rect.minY), Float(rect.width), Float(rect.height)),
                                      color: enabled ? SIMD4(0.22, 0.34, 0.30, 1) : SIMD4(0.20, 0.22, 0.25, 1)))
            instances.append(instance(SIMD4(Float(rect.minX), 0, Float(rect.width), Float(min(3, layout.rulerHeight))),
                                      color: enabled ? SIMD4(0.40, 0.84, 0.56, 1) : SIMD4(0.45, 0.48, 0.53, 1)))
        }
        instances += loopHandles.instances(enabled: piano.loop?.enabled == true, scale: input.scale)
        if let x = startX {
            // A stem with a flag at its top: stop returns here.
            let color = SIMD4<Float>(0.40, 0.84, 0.56, 1)
            let flag = Float(min(8, layout.ruler.maxX - x))
            instances.append(instance(SIMD4(Float(x), 0, 2, Float(layout.rulerHeight)), color: color))
            instances.append(instance(SIMD4(Float(x), 0, flag, Float(min(6, layout.rulerHeight))), color: color))
        }
        return instances
    }

    var loopHandles: RulerHandles {
        RulerHandles(range: piano.loop?.range, ruler: layout.ruler,
                     originBeat: piano.camera.originBeat, pixelsPerBeat: piano.camera.pixelsPerBeat)
    }

    /// The visible part of the loop band, never the keyboard headings or note grid.
    var loopRect: CGRect? {
        guard let range = piano.loop?.range else { return nil }
        let left = layout.gridOrigin.x + (Double(range.start) / 705_600 - piano.camera.originBeat) * piano.camera.pixelsPerBeat
        let right = layout.gridOrigin.x + (Double(range.end) / 705_600 - piano.camera.originBeat) * piano.camera.pixelsPerBeat
        let rect = CGRect(x: max(layout.ruler.minX, left), y: 0,
                          width: max(0, min(layout.ruler.maxX, right) - max(layout.ruler.minX, left)), height: layout.rulerHeight)
        return rect.isEmpty ? nil : rect
    }

    /// Start marker x in points. Nil when it falls outside the ruler.
    var startX: Double? {
        guard let start = piano.start else { return nil }
        let x = (Double(start) / 705_600 - piano.camera.originBeat) * piano.camera.pixelsPerBeat
        guard x >= 0, x < layout.grid.width else { return nil }
        return layout.gridOrigin.x + x
    }

    var labels: [CanvasLabel] {
        let foreground = SIMD4<Float>(0.90, 0.91, 0.94, 1)
        var labels = [label("I–VII", x: 2, y: (layout.rulerHeight - ArrangeLabelRaster.height) / 2,
                            width: max(0, layout.degreeWidth - 4), clip: CGRect(x: 0, y: 0, width: layout.degreeWidth, height: layout.rulerHeight), color: foreground, alignment: .center),
                      label(text("Keyboard"), x: layout.degreeWidth + 4, y: (layout.rulerHeight - ArrangeLabelRaster.height) / 2,
                            width: max(0, layout.keyboardWidth - 8), clip: CGRect(x: layout.degreeWidth, y: 0, width: layout.keyboardWidth, height: layout.rulerHeight), color: foreground)]
        for row in rows {
            let y = Double(row.y) + (Double(row.height) - ArrangeLabelRaster.height) / 2
            if let degree = piano.key.degree(row.pitch), let numeral = DisplayKey.numeral(degree) {
                labels.append(label(numeral, x: 4, y: y, width: max(0, layout.degreeWidth - 8),
                                    clip: CGRect(x: 0, y: max(layout.rulerHeight, Double(row.y)), width: layout.degreeWidth,
                                                 height: max(0, min(input.height, Double(row.y + row.height - 1)) - max(layout.rulerHeight, Double(row.y)))),
                                    color: degree == 1 ? SIMD4(0.96, 0.75, 0.40, 1) : foreground, alignment: .center))
            }
            if row.pitch % 12 == 0 {
                labels.append(label(noteName(row.pitch), x: layout.degreeWidth + 8, y: y, width: max(0, layout.keyboardWidth - 16),
                                    clip: layout.keyboard, color: SIMD4(0.20, 0.22, 0.25, 1)))
            }
        }
        for beat in beats {
            let x = layout.gridOrigin.x + (Double(beat) - piano.camera.originBeat) * piano.camera.pixelsPerBeat + 3
            labels.append(label(String(beat), x: x, y: (layout.rulerHeight - ArrangeLabelRaster.height) / 2,
                                width: min(48, max(0, piano.camera.pixelsPerBeat - 4)), clip: layout.ruler, color: foreground))
        }
        return labels
    }

    private var beats: ClosedRange<Int> {
        let first = Int(piano.camera.originBeat.rounded(.up))
        let last = Int((piano.camera.originBeat + layout.grid.width / piano.camera.pixelsPerBeat).rounded(.up))
        return first...max(first, last)
    }

    private func isBlack(_ pitch: Int) -> Bool { [1, 3, 6, 8, 10].contains(pitch % 12) }
    private func instance(_ rect: SIMD4<Float>, color: SIMD4<Float>) -> Instance { Instance(rect: rect * Float(input.scale), color: color) }
    private func label(_ value: String, x: Double, y: Double, width: Double, clip: CGRect, color: SIMD4<Float>, alignment: CanvasLabelAlignment = .left) -> CanvasLabel {
        CanvasLabel(text: value, rect: SIMD4(Float(x), Float(y), Float(width), Float(ArrangeLabelRaster.height)),
                    clip: SIMD4(Float(clip.minX), Float(clip.minY), Float(clip.width), Float(clip.height)), color: color, alignment: alignment)
    }
}
