import AppKit
import QuartzCore
import Testing
@testable import EchodeAppCore

@MainActor
struct CanvasResizeTests {
    @Test func contractHeaderWidthSurvivesZeroAndNarrowLayouts() {
        let navigation = CanvasNavigationState()
        let pane = ArrangePaneView(frame: NSRect(x: 0, y: 0, width: 900, height: 300))
        pane.setNavigation(navigation)
        pane.layout()
        pane.resizeHeader(to: 220)
        let camera = pane.viewport
        // These dimensions exercise layout constraints, not project limits.
        for (width, visible) in [(0.0, 0.0), (80, 40), (240, 120), (400, 150), (520, 220), (900, 220)] {
            pane.setFrameSize(NSSize(width: width, height: 300))
            pane.layout()
            #expect(Double(pane.headers.frame.width) == visible, "Pane \(width), header \(pane.headers.frame.width), expected \(visible)")
            #expect(pane.timeline.renderInput.header?.width == visible)
            #expect(ArrangeFrame(input: pane.timeline.renderInput).timelineOrigin == min(width, visible + 1))
            #expect(pane.viewport == camera)
            #expect(navigation.headerWidth == 220)
        }
        // Recreating a pane while narrow must not replace the remembered width.
        let restored = ArrangePaneView(frame: NSRect(x: 0, y: 0, width: 400, height: 300))
        restored.setNavigation(navigation)
        restored.layout()
        #expect(restored.headers.frame.width == 150)
        #expect(navigation.headerWidth == 220)
        restored.setFrameSize(NSSize(width: 900, height: 300))
        restored.layout()
        #expect(restored.headers.frame.width == 220)
        // Only an intentional header resize replaces the requested width.
        restored.resizeHeader(to: 180)
        #expect(navigation.headerWidth == 180)
        restored.setFrameSize(.zero)
        restored.layout()
        restored.setFrameSize(NSSize(width: 900, height: 300))
        restored.layout()
        #expect(restored.headers.frame.width == 180)
        let initiallyEmpty = ArrangePaneView(frame: .zero)
        initiallyEmpty.layout()
        initiallyEmpty.setFrameSize(NSSize(width: 900, height: 300))
        initiallyEmpty.layout()
        #expect(initiallyEmpty.headers.frame.width == 160)
    }

    @Test func contractCanvasResizeAnchorsOldContentsAndSizesDrawablesImmediately() throws {
        // No window, display link, audio device, or GPU work is started.
        let arrange = ArrangeView(frame: NSRect(x: 0, y: 0, width: 900, height: 700))
        let piano = PianoGridView(frame: arrange.frame)
        for view in [arrange as NSView, piano as NSView] {
            let layer = try #require(view.layer as? CAMetalLayer)
            #expect(view.layerContentsPlacement == .topLeft)
            #expect(layer.contentsGravity == .topLeft)
            #expect(view.layerContentsRedrawPolicy == .never)
            for size in [NSSize(width: 380, height: 180), .zero, NSSize(width: 900, height: 700),
                         NSSize(width: 640.25, height: 211.75)] {
                view.setFrameSize(size)
                // Inspect before child layout or a render-thread run-loop turn.
                #expect(layer.contentsScale == 1)
                #expect(layer.drawableSize == NSSize(width: max(1, ceil(size.width)), height: max(1, ceil(size.height))))
                #expect(layer.contentsGravity == .topLeft)
                #expect(view.layerContentsPlacement == .topLeft)
                view.layout()
                #expect(layer.drawableSize == NSSize(width: max(1, ceil(size.width)), height: max(1, ceil(size.height))))
            }
            #expect(view.window == nil)
        }
        #expect(!arrange.rendererAvailable && !piano.rendererAvailable)
    }

    @Test func backingScaleAndFractionalSizesRejectOldDrawableExtents() {
        let layer = CAMetalLayer()
        for scale in [1.0, 2.0] {
            let input = CanvasInput(snapshot: nil, selected: nil, camera: ArrangeCamera(), width: 640.25, height: 211.75, scale: scale)
            let expected = CGSize(width: ceil(640.25 * scale), height: ceil(211.75 * scale))
            CanvasSurface.resize(layer, size: input.drawableSize, scale: scale)
            #expect(Double(layer.contentsScale) == scale)
            #expect(layer.drawableSize == expected)
            #expect(input.acceptsDrawable(size: expected))
            #expect(!input.acceptsDrawable(size: CGSize(width: expected.width - 1, height: expected.height)))
            #expect(!input.acceptsDrawable(size: CGSize(width: expected.width, height: expected.height + 1)))
            #expect(!input.acceptsDrawable(size: CGSize(width: 900 * scale, height: 700 * scale)))
            let empty = CanvasInput(snapshot: nil, selected: nil, camera: ArrangeCamera(), width: 0, height: 0, scale: scale)
            CanvasSurface.resize(layer, size: empty.drawableSize, scale: scale)
            #expect(layer.drawableSize == CGSize(width: 1, height: 1))
            #expect(!empty.acceptsDrawable(size: layer.drawableSize))
        }
    }
}
