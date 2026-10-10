import Foundation
import Metal

/// The display renderer and the offscreen GPU probe use the same encoder.
/// Headers and timeline share a command buffer, attachment and presentation.
final class ArrangePainter {
    private let pipeline: any MTLRenderPipelineState
    private let labelPipeline: any MTLRenderPipelineState
    private let labels = ArrangeLabelTextures()
    private let device: any MTLDevice

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
        descriptor.vertexFunction = library.makeFunction(name: "label_vertex")
        descriptor.fragmentFunction = library.makeFunction(name: "label_fragment")
        labelPipeline = try device.makeRenderPipelineState(descriptor: descriptor)
    }

    func encode(encoder: any MTLRenderCommandEncoder, texture: any MTLTexture, buffer: any MTLBuffer,
                timelineCount: Int, headerCount: Int, frame: ArrangeFrame?, piano: PianoFrame? = nil) {
        var viewport = SIMD2(Float(texture.width), Float(texture.height))
        encoder.setRenderPipelineState(pipeline)
        encoder.setVertexBuffer(buffer, offset: 0, index: 0)
        encoder.setVertexBytes(&viewport, length: MemoryLayout<SIMD2<Float>>.stride, index: 1)
        if let frame {
            if let rect = scissor(x: frame.timelineOrigin * frame.input.scale, y: 0,
                                  width: frame.timelineWidth * frame.input.scale, height: Double(texture.height), texture: texture) {
                encoder.setScissorRect(rect)
                encoder.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 6, instanceCount: timelineCount)
            }
            if let rect = scissor(x: 0, y: 0, width: frame.timelineOrigin * frame.input.scale,
                                  height: Double(texture.height), texture: texture) {
                encoder.setScissorRect(rect)
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

    private func encodeLabels(_ frame: ArrangeFrame, encoder: any MTLRenderCommandEncoder, texture: any MTLTexture) {
        guard let header = frame.input.header else { return }
        let width = max(0, header.width - 38)
        let scale = frame.input.scale
        let text = frame.headers.map { row in
            CanvasLabel(text: row.name,
                        rect: SIMD4(30, row.y + (row.height - Float(ArrangeLabelRaster.height)) / 2, Float(width), Float(ArrangeLabelRaster.height)),
                        clip: SIMD4(30, row.y, Float(width), max(0, row.height - 1)),
                        color: row.id == frame.input.selected ? header.palette.selectedForeground : header.palette.foreground)
        }
        encodeLabels(text, scale: scale, encoder: encoder, texture: texture)
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
