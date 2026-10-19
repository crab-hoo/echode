import Foundation
import Metal

/// The display renderer and the offscreen GPU probe use the same encoder.
/// Headers and timeline share a command buffer, attachment and presentation.
struct WaveInstance {
    var rect: SIMD4<Float>
    var color: SIMD4<Float>
    var offset: UInt32
    var columns: UInt32
    var pad0: UInt32 = 0
    var pad1: UInt32 = 0
}

final class ArrangePainter {
    private let pipeline: any MTLRenderPipelineState
    private let wavePipeline: any MTLRenderPipelineState
    private let labelPipeline: any MTLRenderPipelineState
    private let labels = ArrangeLabelTextures()
    private let device: any MTLDevice
    /// One pair per in-flight drawable. The probe uses slot 0 and waits.
    private var waveInstances: [MTLBuffer?] = [nil, nil, nil]
    private var wavePeaks: [MTLBuffer?] = [nil, nil, nil]

    init(device: any MTLDevice) throws {
        self.device = device
        let library = try device.makeLibrary(source: arrangeShader, options: nil)
        let descriptor = MTLRenderPipelineDescriptor()
        descriptor.vertexFunction = library.makeFunction(name: "arrange_vertex")
        descriptor.fragmentFunction = library.makeFunction(name: "arrange_fragment")
        descriptor.colorAttachments[0].pixelFormat = .bgra8Unorm
        descriptor.colorAttachments[0].isBlendingEnabled = true
        descriptor.colorAttachments[0].sourceRGBBlendFactor = .sourceAlpha
        descriptor.colorAttachments[0].destinationRGBBlendFactor = .oneMinusSourceAlpha
        descriptor.colorAttachments[0].sourceAlphaBlendFactor = .one
        descriptor.colorAttachments[0].destinationAlphaBlendFactor = .oneMinusSourceAlpha
        pipeline = try device.makeRenderPipelineState(descriptor: descriptor)
        descriptor.vertexFunction = library.makeFunction(name: "wave_vertex")
        descriptor.fragmentFunction = library.makeFunction(name: "wave_fragment")
        wavePipeline = try device.makeRenderPipelineState(descriptor: descriptor)
        descriptor.vertexFunction = library.makeFunction(name: "label_vertex")
        descriptor.fragmentFunction = library.makeFunction(name: "label_fragment")
        labelPipeline = try device.makeRenderPipelineState(descriptor: descriptor)
    }

    func encode(encoder: any MTLRenderCommandEncoder, texture: any MTLTexture, buffer: any MTLBuffer,
                timelineCount: Int, headerCount: Int, frame: ArrangeFrame?, piano: PianoFrame? = nil, slot: Int = 0) {
        var viewport = SIMD2(Float(texture.width), Float(texture.height))
        encoder.setRenderPipelineState(pipeline)
        encoder.setVertexBuffer(buffer, offset: 0, index: 0)
        encoder.setVertexBytes(&viewport, length: MemoryLayout<SIMD2<Float>>.stride, index: 1)
        if let frame {
            if let rect = scissor(x: frame.timelineOrigin * frame.input.scale, y: 0,
                                  width: frame.timelineWidth * frame.input.scale, height: Double(texture.height), texture: texture) {
                encoder.setScissorRect(rect)
                let split = min(frame.foregroundStart, timelineCount)
                encoder.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 6, instanceCount: split)
                encodeWaveforms(frame, encoder: encoder, texture: texture, slot: slot)
                if timelineCount > split {
                    encoder.setRenderPipelineState(pipeline)
                    encoder.setVertexBuffer(buffer, offset: 0, index: 0)
                    encoder.setVertexBytes(&viewport, length: MemoryLayout<SIMD2<Float>>.stride, index: 1)
                    encoder.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 6, instanceCount: timelineCount - split, baseInstance: split)
                }
            }
            if let rect = scissor(x: 0, y: 0, width: frame.timelineOrigin * frame.input.scale,
                                  height: Double(texture.height), texture: texture) {
                encoder.setScissorRect(rect)
                encoder.setRenderPipelineState(pipeline)
                encoder.setVertexBuffer(buffer, offset: 0, index: 0)
                encoder.setVertexBytes(&viewport, length: MemoryLayout<SIMD2<Float>>.stride, index: 1)
                encoder.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 6, instanceCount: headerCount, baseInstance: timelineCount)
            }
            encodeLabels(frame, encoder: encoder, texture: texture)
        } else if let piano {
            let scale = piano.input.scale
            let rulerCount = piano.rulerInstances.count
            let sidebarCount = headerCount - rulerCount
            if let clip = scissor(piano.layout.grid, scale: scale, texture: texture) {
                encoder.setScissorRect(clip)
                encoder.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 6, instanceCount: timelineCount)
            }
            if let clip = scissor(CGRect(x: 0, y: piano.layout.rulerHeight, width: piano.layout.gridOrigin.x, height: piano.layout.grid.height), scale: scale, texture: texture) {
                encoder.setScissorRect(clip)
                encoder.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 6, instanceCount: sidebarCount, baseInstance: timelineCount)
            }
            if let clip = scissor(CGRect(x: 0, y: 0, width: piano.input.width, height: piano.layout.rulerHeight), scale: scale, texture: texture) {
                encoder.setScissorRect(clip)
                encoder.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 6, instanceCount: rulerCount, baseInstance: timelineCount + sidebarCount)
            }
            encodeLabels(piano.labels, scale: scale, encoder: encoder, texture: texture)
        } else {
            encoder.setScissorRect(MTLScissorRect(x: 0, y: 0, width: texture.width, height: texture.height))
            encoder.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 6, instanceCount: timelineCount)
        }
    }

    private func encodeWaveforms(_ frame: ArrangeFrame, encoder: any MTLRenderCommandEncoder, texture: any MTLTexture, slot: Int) {
        let draws = frame.waveforms
        guard !draws.isEmpty else { return }
        var instances: [WaveInstance] = []
        var peaks: [SIMD2<Float>] = []
        instances.reserveCapacity(draws.count)
        for draw in draws where !draw.columns.isEmpty {
            instances.append(WaveInstance(rect: draw.rect, color: draw.color, offset: UInt32(peaks.count), columns: UInt32(draw.columns.count)))
            peaks.append(contentsOf: draw.columns)
        }
        guard !instances.isEmpty else { return }
        let slot = min(max(0, slot), waveInstances.count - 1)
        guard let instancesBuffer = reusedBuffer(&waveInstances[slot], bytes: instances),
              let peaksBuffer = reusedBuffer(&wavePeaks[slot], bytes: peaks) else { return }
        var viewport = SIMD2(Float(texture.width), Float(texture.height))
        encoder.setRenderPipelineState(wavePipeline)
        encoder.setVertexBuffer(instancesBuffer, offset: 0, index: 0)
        encoder.setVertexBytes(&viewport, length: MemoryLayout<SIMD2<Float>>.stride, index: 1)
        encoder.setFragmentBuffer(peaksBuffer, offset: 0, index: 0)
        encoder.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 6, instanceCount: instances.count)
    }

    /// Grows a shared buffer and copies into it. The slot stays alive until that
    /// frame's command buffer completes, so the next frame uses another slot.
    private func reusedBuffer<T>(_ slot: inout (any MTLBuffer)?, bytes: [T]) -> (any MTLBuffer)? {
        let length = max(1, bytes.count * MemoryLayout<T>.stride)
        if (slot?.length ?? 0) < length {
            slot = device.makeBuffer(length: length, options: .storageModeShared)
        }
        guard let buffer = slot else { return nil }
        bytes.withUnsafeBytes { raw in
            if let source = raw.baseAddress { buffer.contents().copyMemory(from: source, byteCount: raw.count) }
        }
        return buffer
    }

    private func encodeLabels(_ frame: ArrangeFrame, encoder: any MTLRenderCommandEncoder, texture: any MTLTexture) {
        guard let header = frame.input.header else { return }
        let width = max(0, header.width - 65)
        let scale = frame.input.scale
        let text = frame.headers.flatMap { row -> [CanvasLabel] in
            let layout = TrackControlLayout(row: CGRect(x: 0, y: Double(row.y), width: header.width, height: Double(row.height)))
            let fx = TrackFXLayout(row: layout.row)
            let color = row.id == frame.input.selected ? header.palette.selectedForeground : header.palette.foreground
            let fxLabels = fx.visible ? [CanvasLabel(text: "FX", rect: SIMD4(Float(fx.rect.minX + 3), Float(fx.rect.minY), Float(fx.rect.width - 3), Float(fx.rect.height)),
                clip: SIMD4(Float(fx.rect.minX), Float(fx.rect.minY), Float(fx.rect.width), Float(fx.rect.height)), color: color)] : []
            if layout.visible {
                var labels = [CanvasLabel(text: row.name,
                    rect: SIMD4(8, row.y + 4, Float(max(0, header.width - 45)), Float(ArrangeLabelRaster.height)),
                    clip: SIMD4(8, row.y, Float(max(0, header.width - 45)), 27), color: color)]
                for (label, rect) in [("M", layout.mute), ("S", layout.solo)] {
                    labels.append(CanvasLabel(text: label, rect: SIMD4(Float(rect.minX + 4), Float(rect.minY), Float(rect.width - 4), Float(rect.height)),
                        clip: SIMD4(Float(rect.minX), Float(rect.minY), Float(rect.width), Float(rect.height)), color: color))
                }
                return labels + fxLabels
            }
            return [CanvasLabel(text: row.name,
                        rect: SIMD4(30, row.y + (row.height - Float(ArrangeLabelRaster.height)) / 2, Float(width), Float(ArrangeLabelRaster.height)),
                        clip: SIMD4(30, row.y, Float(width), max(0, row.height - 1)),
                        color: color)] + fxLabels
        }
        encodeLabels(text, scale: scale, encoder: encoder, texture: texture)
        if let layout = frame.clipControlLayout, let id = frame.input.selectedClip, let clip = frame.input.snapshot?.clipMix(id) {
            let gain = frame.input.clipDraft?.gain ?? clip.gain
            let rect = SIMD4(Float(layout.gain.midX - 35), Float(layout.gain.maxY + 2), 80, 18)
            encodeLabels([CanvasLabel(text: MixScale.label(gain), rect: rect,
                clip: SIMD4(Float(max(frame.timelineOrigin, layout.rect.minX)), Float(layout.rect.minY),
                            Float(max(0, min(frame.input.width, layout.rect.maxX) - max(frame.timelineOrigin, layout.rect.minX))), Float(layout.rect.height)),
                color: SIMD4(1, 1, 1, 1))], scale: scale, encoder: encoder, texture: texture)
        }
    }

    private func encodeLabels(_ text: [CanvasLabel], scale: Double, encoder: any MTLRenderCommandEncoder, texture: any MTLTexture) {
        let textures = labels.prepare(labels: text, scale: scale, device: device)
        encoder.setRenderPipelineState(labelPipeline)
        for item in text {
            guard let label = textures[item.key(scale: scale)],
                  let clip = scissor(x: Double(item.clip.x) * scale, y: Double(item.clip.y) * scale,
                                     width: Double(item.clip.z) * scale, height: Double(item.clip.w) * scale, texture: texture) else { continue }
            var quad = Instance(rect: item.rect * Float(scale), color: item.color)
            encoder.setScissorRect(clip)
            encoder.setVertexBytes(&quad, length: MemoryLayout<Instance>.stride, index: 0)
            encoder.setFragmentTexture(label, index: 0)
            encoder.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 6)
        }
    }

    private func scissor(_ rect: CGRect, scale: Double, texture: any MTLTexture) -> MTLScissorRect? {
        scissor(x: rect.minX * scale, y: rect.minY * scale, width: rect.width * scale, height: rect.height * scale, texture: texture)
    }

    private func scissor(x: Double, y: Double, width: Double, height: Double, texture: any MTLTexture) -> MTLScissorRect? {
        let left = max(0, min(texture.width, Int(x.rounded(.up))))
        let top = max(0, min(texture.height, Int(y.rounded(.up))))
        let right = max(0, min(texture.width, Int((x + width).rounded(.up))))
        let bottom = max(0, min(texture.height, Int((y + height).rounded(.up))))
        guard right > left, bottom > top else { return nil }
        return MTLScissorRect(x: left, y: top, width: right - left, height: bottom - top)
    }
}
