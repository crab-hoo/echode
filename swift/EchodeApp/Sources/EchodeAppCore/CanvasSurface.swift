import AppKit
import QuartzCore

/// Use whole backing pixels, including fractional-point pane sizes.
func canvasDrawableSize(width: Double, height: Double, scale: Double) -> CGSize {
    CGSize(width: max(1, ceil(width * scale)), height: max(1, ceil(height * scale)))
}

/// AppKit owns layer geometry. The render thread only acquires and presents
/// drawables; it never resizes the surface after a later main-thread layout.
@MainActor
enum CanvasSurface {
    static func configure(_ view: NSView, layer: CAMetalLayer) {
        view.layerContentsRedrawPolicy = .never
        // AppKit converts this to the visual top-left; flipped views need
        // `bottomLeft` because CA's `top` points at the visual bottom.
        view.layerContentsPlacement = .topLeft
        view.wantsLayer = true
        layer.backgroundColor = CanvasBackground.layerColor
        resize(
            layer, size: canvasDrawableSize(width: view.bounds.width, height: view.bounds.height, scale: 1), scale: 1)
    }

    static func resize(_ layer: CAMetalLayer, size: CGSize, scale: Double) {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        if Double(layer.contentsScale) != scale { layer.contentsScale = scale }
        if layer.drawableSize != size { layer.drawableSize = size }
        CATransaction.commit()
    }
}
