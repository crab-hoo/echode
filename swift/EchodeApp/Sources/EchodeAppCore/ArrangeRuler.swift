import AppKit

/// The ruler belongs to the arrange's single Metal surface, not a second canvas.
struct ArrangeRulerInput: Sendable, Equatable {
    static let height = 24.0
    var start: Int64?
    var loop: RulerLoop?
}

extension ArrangeView {
    func setRuler(start: Int64?, loop: RulerLoop?, busy: Bool) {
        let next = ArrangeRulerInput(start: start, loop: loop)
        guard ruler != next || rulerBusy != busy else { return }
        if ruler?.loop != next.loop { rulerPress = nil }
        ruler = next; rulerBusy = busy
        if busy { rulerPress = nil }
        refresh()
    }

    var displayedLoop: RulerLoop? {
        if let range = rulerPress?.range { return RulerLoop(range: range, enabled: ruler?.loop?.enabled ?? false) }
        return ruler?.loop
    }

    var rulerHandles: RulerHandles {
        let input = renderInput
        let origin = input.header.map { $0.width + 1 } ?? 0
        return RulerHandles(range: displayedLoop?.range,
            ruler: CGRect(x: origin, y: 0, width: max(0, input.width - origin), height: input.arrangeTop),
            originBeat: camera.originBeat, pixelsPerBeat: camera.pixelsPerBeat)
    }

    func cancelRuler() {
        guard rulerPress != nil else { return }
        rulerPress = nil; refresh()
    }

    /// Coordinates are relative to the timeline, excluding the track headers.
    func pressRuler(at point: CGPoint) -> Bool {
        let input = renderInput
        let origin = input.header.map { $0.width + 1 } ?? 0
        let rect = CGRect(x: origin, y: 0, width: max(0, input.width - origin), height: input.arrangeTop)
        guard point.y >= 0, point.y < input.arrangeTop else { return false }
        cancelRuler()
        guard !rulerBusy, snapshot != nil, rect.contains(point) else { return true }
        rulerPress = RulerPress(x: point.x - origin, originBeat: camera.originBeat,
                                pixelsPerBeat: camera.pixelsPerBeat, grid: 705_600,
                                edge: rulerHandles.hit(at: point), originalRange: ruler?.loop?.range)
        return true
    }

    func dragRuler(to point: CGPoint, flags: NSEvent.ModifierFlags) {
        guard var press = rulerPress, !rulerBusy else { cancelRuler(); return }
        let origin = renderInput.header.map { $0.width + 1 } ?? 0
        press.update(x: point.x - origin, snapping: !flags.contains(.shift))
        rulerPress = press; refresh()
    }

    func finishRuler(at point: CGPoint, flags: NSEvent.ModifierFlags) {
        dragRuler(to: point, flags: flags)
        guard let press = rulerPress else { return }
        cancelRuler()
        guard !rulerBusy else { return }
        if press.dragged {
            if let range = press.range { onSetLoop?(range) }
        } else if press.edge == nil, let tick = press.tick(at: press.x, snapping: !flags.contains(.shift)) {
            onSetStart?(tick)
        }
    }
}

extension ArrangeFrame {
    var rulerRect: CGRect {
        CGRect(x: timelineOrigin, y: 0, width: timelineWidth, height: input.arrangeTop)
    }

    var loopRect: CGRect? {
        guard let range = input.arrangeRuler?.loop?.range else { return nil }
        let left = timelineOrigin + (Double(range.start) / 705_600 - camera.originBeat) * camera.pixelsPerBeat
        let right = timelineOrigin + (Double(range.end) / 705_600 - camera.originBeat) * camera.pixelsPerBeat
        let rect = CGRect(x: max(timelineOrigin, left), y: 0,
                          width: max(0, min(input.width, right) - max(timelineOrigin, left)), height: input.arrangeTop)
        return rect.isEmpty ? nil : rect
    }

    var startX: Double? {
        guard let start = input.arrangeRuler?.start else { return nil }
        let x = (Double(start) / 705_600 - camera.originBeat) * camera.pixelsPerBeat
        return x >= 0 && x < timelineWidth ? timelineOrigin + x : nil
    }

    var loopHandles: RulerHandles {
        RulerHandles(range: input.arrangeRuler?.loop?.range, ruler: rulerRect,
                     originBeat: camera.originBeat, pixelsPerBeat: camera.pixelsPerBeat)
    }

    var rulerInstances: [Instance] {
        guard input.arrangeRuler != nil else { return [] }
        func instance(_ rect: SIMD4<Float>, _ color: SIMD4<Float>) -> Instance {
            Instance(rect: rect * Float(input.scale), color: color)
        }
        let height = Float(input.arrangeTop)
        var result = [instance(SIMD4(0, 0, Float(input.width), height), SIMD4(0.12, 0.13, 0.16, 1)),
                      instance(SIMD4(0, max(0, height - 1), Float(input.width), 1), SIMD4(0.27, 0.29, 0.33, 1))]
        if let rect = loopRect {
            let enabled = input.arrangeRuler?.loop?.enabled == true
            result.append(instance(SIMD4(Float(rect.minX), 0, Float(rect.width), height),
                                   enabled ? SIMD4(0.22, 0.34, 0.30, 1) : SIMD4(0.20, 0.22, 0.25, 1)))
            result.append(instance(SIMD4(Float(rect.minX), 0, Float(rect.width), min(3, height)),
                                   enabled ? SIMD4(0.40, 0.84, 0.56, 1) : SIMD4(0.45, 0.48, 0.53, 1)))
        }
        result += loopHandles.instances(enabled: input.arrangeRuler?.loop?.enabled == true, scale: input.scale)
        if let x = startX {
            let color = SIMD4<Float>(0.40, 0.84, 0.56, 1)
            result.append(instance(SIMD4(Float(x), 0, Float(min(2, input.width - x)), height), color))
            result.append(instance(SIMD4(Float(x), 0, Float(min(8, input.width - x)), min(6, height)), color))
        }
        return result
    }

    var rulerLabels: [CanvasLabel] {
        guard input.arrangeRuler != nil, timelineWidth > 0 else { return [] }
        let first = Int(camera.originBeat.rounded(.up))
        let last = max(first, Int((camera.originBeat + timelineWidth / camera.pixelsPerBeat).rounded(.up)))
        return (first...last).map { beat in
            let x = timelineOrigin + (Double(beat) - camera.originBeat) * camera.pixelsPerBeat + 3
            return CanvasLabel(text: String(beat), rect: SIMD4(Float(x), Float((input.arrangeTop - ArrangeLabelRaster.height) / 2),
                Float(min(48, max(0, camera.pixelsPerBeat - 4))), Float(ArrangeLabelRaster.height)),
                clip: SIMD4(Float(timelineOrigin), 0, Float(timelineWidth), Float(input.arrangeTop)), color: SIMD4(0.90, 0.91, 0.94, 1))
        }
    }
}
