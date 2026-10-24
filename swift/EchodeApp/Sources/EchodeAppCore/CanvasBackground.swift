import CoreGraphics
import Metal

/// The surface background shared by the Metal clear, the offscreen probe and
/// the layer behind an exposed drawable margin. The clear color and the layer
/// color stay in the same sRGB values so a resize does not flash transparent.
enum CanvasBackground {
    private static let components = (red: 0.09, green: 0.10, blue: 0.12)
    static var clearColor: MTLClearColor {
        MTLClearColor(red: components.red, green: components.green, blue: components.blue, alpha: 1)
    }
    static var layerColor: CGColor {
        CGColor(srgbRed: components.red, green: components.green, blue: components.blue, alpha: 1)
    }
}
