import CoreGraphics
import CoreText
import Foundation
import Metal

enum CanvasLabelAlignment: Sendable, Hashable { case left, center }

struct CanvasLabel: Sendable {
    let text: String
    let rect: SIMD4<Float>
    let clip: SIMD4<Float>
    let color: SIMD4<Float>
    var alignment = CanvasLabelAlignment.left

    func key(scale: Double) -> CanvasLabelKey { CanvasLabelKey(text: text, width: Double(rect.z), scale: scale, alignment: alignment) }
}

struct CanvasLabelKey: Hashable {
    let text: String
    let width: Double
    let scale: Double
    let alignment: CanvasLabelAlignment
}

/// Core Text handles shaping, fallback fonts and truncation. This bitmap is a
/// white coverage mask, tinted by Metal in the same frame as the track lane.
struct ArrangeLabelRaster {
    static let height = 20.0
    let width: Int
    let height: Int
    let pixels: [UInt8]

    init?(name: String, width: Double, scale: Double, alignment: CanvasLabelAlignment = .left) {
        guard width > 0, width.isFinite, scale > 0, scale.isFinite else { return nil }
        let pixelWidth = Int((width * scale).rounded(.up))
        let pixelHeight = Int((Self.height * scale).rounded(.up))
        guard let context = CGContext(data: nil, width: pixelWidth, height: pixelHeight,
                                      bitsPerComponent: 8, bytesPerRow: pixelWidth,
                                      space: CGColorSpaceCreateDeviceGray(), bitmapInfo: CGImageAlphaInfo.none.rawValue),
              let data = context.data,
              let font = CTFontCreateUIFontForLanguage(.system, 12, nil) else { return nil }
        data.initializeMemory(as: UInt8.self, repeating: 0, count: pixelWidth * pixelHeight)
        context.scaleBy(x: scale, y: scale)
        context.setShouldSmoothFonts(false)
        let attributes: [NSAttributedString.Key: Any] = [
            NSAttributedString.Key(kCTFontAttributeName as String): font,
            NSAttributedString.Key(kCTForegroundColorAttributeName as String): CGColor(gray: 1, alpha: 1),
        ]
        let line = CTLineCreateWithAttributedString(NSAttributedString(string: name, attributes: attributes))
        let ellipsis = CTLineCreateWithAttributedString(NSAttributedString(string: "…", attributes: attributes))
        let truncated = CTLineCreateTruncatedLine(line, width, .end, ellipsis) ?? line
        let baseline = (Self.height - Double(CTFontGetAscent(font) + CTFontGetDescent(font))) / 2 + Double(CTFontGetDescent(font))
        let lineWidth = CTLineGetTypographicBounds(truncated, nil, nil, nil)
        context.textPosition = CGPoint(x: alignment == .center ? max(0, (width - lineWidth) / 2) : 0, y: baseline)
        CTLineDraw(truncated, context)
        self.width = pixelWidth
        height = pixelHeight
        pixels = Array(UnsafeBufferPointer(start: data.assumingMemoryBound(to: UInt8.self), count: pixelWidth * pixelHeight))
    }
}

/// Render-thread only. Published textures are immutable; command buffers retain
/// old textures through completion when names, width or display scale change.
final class ArrangeLabelTextures {
    private var textures: [CanvasLabelKey: any MTLTexture] = [:]

    func prepare(labels: [CanvasLabel], scale: Double, device: any MTLDevice) -> [CanvasLabelKey: any MTLTexture] {
        var visible: [CanvasLabelKey: any MTLTexture] = [:]
        for label in labels {
            let key = label.key(scale: scale)
            if let texture = textures[key] ?? visible[key] { visible[key] = texture; continue }
            guard let raster = ArrangeLabelRaster(name: key.text, width: key.width, scale: scale, alignment: key.alignment) else { continue }
            let descriptor = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .r8Unorm, width: raster.width, height: raster.height, mipmapped: false)
            descriptor.storageMode = .shared
            descriptor.usage = .shaderRead
            guard let texture = device.makeTexture(descriptor: descriptor) else { continue }
            raster.pixels.withUnsafeBytes { pixels in
                texture.replace(region: MTLRegionMake2D(0, 0, raster.width, raster.height), mipmapLevel: 0,
                                withBytes: pixels.baseAddress!, bytesPerRow: raster.width)
            }
            visible[key] = texture
        }
        // Retain only visible names, not every track in the project.
        textures = visible
        return visible
    }
}
