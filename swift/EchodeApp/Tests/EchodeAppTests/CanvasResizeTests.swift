import AppKit
import QuartzCore
import Testing
@testable import EchodeAppCore

@MainActor
struct CanvasResizeTests {
    @Test func contractHeaderWidthSurvivesZeroAndNarrowLayouts() {
        let navigation = CanvasNavigationState()
        let pane = ArrangePaneView(frame: NSRect(x: 0, y: 0, width: 900, height: 300))
        pane.setNavigation(navigation, documentID: navigation.document)
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
        restored.setNavigation(navigation, documentID: navigation.document)
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
            // A flipped view anchors the old frame at the visual top-left. CA's
            // `top` is the visual bottom, so AppKit converts the placement.
            #expect(layer.contentsAreFlipped())
            #expect(layer.contentsGravity == .bottomLeft)
            #expect(view.layerContentsRedrawPolicy == .never)
            // A drawable that has not caught up exposes this opaque layer.
            #expect(layer.backgroundColor == CanvasBackground.layerColor)
            for size in [NSSize(width: 380, height: 180), .zero, NSSize(width: 900, height: 700),
                         NSSize(width: 640.25, height: 211.75)] {
                view.setFrameSize(size)
                // Inspect before child layout or a render-thread run-loop turn.
                #expect(layer.contentsScale == 1)
                #expect(layer.drawableSize == NSSize(width: max(1, ceil(size.width)), height: max(1, ceil(size.height))))
                #expect(layer.contentsGravity == (layer.contentsAreFlipped() ? .bottomLeft : .topLeft))
                #expect(view.layerContentsPlacement == .topLeft)
                view.layout()
                #expect(layer.drawableSize == NSSize(width: max(1, ceil(size.width)), height: max(1, ceil(size.height))))
            }
            #expect(view.window == nil)
        }
        // The same conversion holds inside the production flipped hierarchy.
        let pane = ArrangePaneView(frame: NSRect(x: 0, y: 0, width: 900, height: 700))
        pane.layout()
        let roll = PianoRollView(frame: NSRect(x: 0, y: 0, width: 900, height: 330))
        roll.layout()
        for surface in [pane.timeline, roll.grid] {
            let layer = try #require(surface.layer as? CAMetalLayer)
            #expect(surface.window == nil && surface.superview != nil)
            #expect(layer.contentsAreFlipped())
            #expect(layer.contentsGravity == .bottomLeft)
        }
        #expect(!arrange.rendererAvailable && !piano.rendererAvailable)
    }

    @Test func contractArrangeCameraKeepsRequestedVerticalPosition() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-camera-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        for index in 0..<10 { await controller.addTrack(name: "t\(index)") }
        let snapshot = try #require(controller.snapshot)
        let navigation = CanvasNavigationState()
        let pane = ArrangePaneView(frame: NSRect(x: 0, y: 0, width: 900, height: 320))
        pane.setNavigation(navigation, documentID: navigation.document)
        pane.setSnapshot(snapshot, selected: nil)
        pane.layout()
        // Scroll to the last track at a short height, then grow the pane.
        pane.timeline.onScroll?(0, -100_000)
        let requested = pane.viewport.originTrack
        #expect(requested > 0)
        #expect(pane.timeline.renderInput.camera.originTrack == requested)
        pane.setFrameSize(NSSize(width: 900, height: 700))
        pane.layout()
        #expect(pane.viewport.originTrack == requested)
        #expect(pane.timeline.renderInput.camera.originTrack < requested)
        // Horizontal input and selecting a visible track leave the remembered origin alone.
        pane.timeline.onScroll?(-120, 0)
        #expect(pane.viewport.originTrack == requested)
        pane.timeline.onZoom?(1.2, [], CGPoint(x: 100, y: 100))
        #expect(pane.viewport.originTrack == requested)
        pane.setSnapshot(snapshot, selected: snapshot.trackID(at: 0))
        #expect(pane.viewport.originTrack == requested)
        // Growing clamped the display only; shrinking returns to the requested origin.
        for height in [0.0, 320.0] {
            pane.setFrameSize(NSSize(width: 900, height: height))
            pane.layout()
            #expect(pane.viewport.originTrack == requested)
            #expect(pane.timeline.renderInput.camera.originTrack == requested, "height \(height)")
        }
        await controller.close()
    }

    @Test func contractArrangeCameraUsesDisplayedRowsForVerticalInput() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-camera-input-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        for index in 0..<20 { await controller.addTrack(name: "t\(index)") }
        let snapshot = try #require(controller.snapshot)
        func pane() -> ArrangePaneView {
            let pane = ArrangePaneView(frame: NSRect(x: 0, y: 0, width: 900, height: 320))
            pane.setNavigation(CanvasNavigationState(), documentID: nil)
            pane.setSnapshot(snapshot, selected: nil)
            pane.layout()
            pane.timeline.onScroll?(0, -100_000)
            pane.setFrameSize(NSSize(width: 900, height: 500))
            pane.layout()
            return pane
        }
        // Scrolling down starts from the displayed rows, not the remembered 5.
        let scroller = pane()
        let shown = scroller.timeline.renderInput.camera.originTrack
        #expect(abs(shown - 12.5625) < 0.001)
        scroller.timeline.onScroll?(0, 64)
        let moved = scroller.timeline.renderInput.camera.originTrack
        #expect(abs(moved - (shown - 1)) < 0.001)
        #expect(abs(scroller.viewport.originTrack - moved) < 0.001)
        // A vertical zoom anchors on the displayed rows as well.
        let zoomer = pane()
        let beforeZoom = zoomer.timeline.renderInput.camera.originTrack
        zoomer.timeline.onZoom?(2, [.option], CGPoint(x: 100, y: 0))
        #expect(abs(zoomer.viewport.originTrack - beforeZoom) < 0.001)
        await controller.close()
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
