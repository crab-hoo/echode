import Foundation

enum LoopEdge: CaseIterable { case start, end }

struct RulerHandle {
    let edge: LoopEdge
    let x: Double
    let hitRect: CGRect
}

/// Only actual, visible endpoints have grips. A clipped band edge is not a loop edge.
struct RulerHandles {
    let range: LoopRange?
    let ruler: CGRect
    let originBeat: Double
    let pixelsPerBeat: Double

    var handles: [RulerHandle] {
        guard let range, !ruler.isEmpty else { return [] }
        return LoopEdge.allCases.compactMap { edge in
            let tick = edge == .start ? range.start : range.end
            let x = ruler.minX + (Double(tick) / 705_600 - originBeat) * pixelsPerBeat
            guard x >= ruler.minX, x <= ruler.maxX else { return nil }
            let hit = CGRect(x: x - 5, y: ruler.minY, width: 10, height: min(8, ruler.height)).intersection(ruler)
            return hit.isEmpty ? nil : RulerHandle(edge: edge, x: x, hitRect: hit)
        }
    }

    func hit(at point: CGPoint) -> LoopEdge? {
        handles.filter { $0.hitRect.contains(point) }
            .min { abs($0.x - point.x) < abs($1.x - point.x) }?.edge
    }

    func instances(enabled: Bool, scale: Double) -> [Instance] {
        let color = enabled ? SIMD4<Float>(0.55, 0.94, 0.67, 1) : SIMD4<Float>(0.65, 0.68, 0.73, 1)
        return handles.flatMap { handle in
            let capX = handle.edge == .start ? handle.x : handle.x - 6
            return [CGRect(x: handle.x - 1, y: ruler.minY, width: 2, height: min(8, ruler.height)),
                    CGRect(x: capX, y: ruler.minY, width: 6, height: min(3, ruler.height))].compactMap { rect -> Instance? in
                let clipped = rect.intersection(ruler)
                guard !clipped.isEmpty else { return nil }
                return Instance(rect: SIMD4(Float(clipped.minX), Float(clipped.minY), Float(clipped.width), Float(clipped.height)) * Float(scale), color: color)
            }
        }
    }
}
