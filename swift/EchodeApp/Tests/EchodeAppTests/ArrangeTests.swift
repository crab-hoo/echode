import AppKit
import EchodeBridge
import Foundation
import Metal
import Testing

@testable import EchodeAppCore

@MainActor
struct ArrangeTests {
    @Test func displayLinkFollowsTheScreenInsteadOfAFixedSixty() {
        let sixty = displayFrameRateRange(maximumFramesPerSecond: 60)
        #expect(sixty.minimum == 60 && sixty.maximum == 60 && sixty.preferred == 60)
        let promotion = displayFrameRateRange(maximumFramesPerSecond: 120)
        #expect(promotion.minimum == 60 && promotion.maximum == 120 && promotion.preferred == 120)
        let slow = displayFrameRateRange(maximumFramesPerSecond: 40)
        #expect(slow.minimum == 40 && slow.maximum == 40 && slow.preferred == 40)
    }

    @Test func linkPauseKeepsRetryingAStaleDrawable() {
        #expect(
            canvasLinkPaused(
                ready: true, continuous: false, metersActive: false, playing: false,
                revisionDrawn: true, staleDrawable: false))
        #expect(
            !canvasLinkPaused(
                ready: true, continuous: false, metersActive: false, playing: false,
                revisionDrawn: true, staleDrawable: true))
        #expect(
            !canvasLinkPaused(
                ready: true, continuous: false, metersActive: false, playing: true,
                revisionDrawn: true, staleDrawable: false))
        #expect(
            !canvasLinkPaused(
                ready: true, continuous: true, metersActive: false, playing: false,
                revisionDrawn: true, staleDrawable: false))
        #expect(
            !canvasLinkPaused(
                ready: true, continuous: false, metersActive: true, playing: false,
                revisionDrawn: true, staleDrawable: false))
        #expect(
            !canvasLinkPaused(
                ready: true, continuous: false, metersActive: false, playing: false,
                revisionDrawn: false, staleDrawable: false))
        #expect(
            canvasLinkPaused(
                ready: false, continuous: false, metersActive: false, playing: false,
                revisionDrawn: true, staleDrawable: false))
    }

    @Test func fractionalRowsHaveNoIndependentInsetAndHitHalfOpenBoundaries() {
        let camera = ArrangeCamera(originTrack: 2.25)
        let rows = camera.rows(trackCount: 20, height: 128)
        #expect(rows.visible == 2..<5)
        #expect(rows.rect(at: 2, width: 160).minY == -16)
        #expect(rows.rect(at: 3, width: 160).minY == 48)
        #expect(rows.rect(at: 3, width: 160).height == 64)
        #expect(rows.index(at: 0) == 2)
        #expect(rows.index(at: 47.9) == 2)
        #expect(rows.index(at: 48) == 3)
        #expect(rows.index(at: 127.9) == 4)
        #expect(rows.index(at: 128) == nil)
        #expect(rows.index(at: -1) == nil)
        #expect(rows.index(at: .infinity) == nil)
    }

    @Test func scrollingAndZoomKeepTheTrackUnderThePointer() {
        var camera = ArrangeCamera(originBeat: 2, originTrack: 7.25)
        camera.scroll(x: -72, y: -32, trackCount: 50, height: 220)
        #expect(camera.originBeat == 3 && camera.originTrack == 7.75)
        let anchor = CGPoint(x: 144, y: 64)
        let anchoredBeat = camera.originBeat + Double(anchor.x) / camera.pixelsPerBeat
        let anchoredTrack = camera.originTrack + Double(anchor.y) / camera.pixelsPerTrack
        camera.zoom(factor: 1.5, horizontal: true, vertical: true, anchor: anchor, trackCount: 50, height: 220)
        #expect(camera.pixelsPerTrack == 96 && camera.pixelsPerBeat == 108)
        #expect(abs(camera.originBeat + Double(anchor.x) / camera.pixelsPerBeat - anchoredBeat) < 1e-10)
        #expect(abs(camera.originTrack + Double(anchor.y) / camera.pixelsPerTrack - anchoredTrack) < 1e-10)
        camera.scroll(x: 1_000_000, y: 1_000_000, trackCount: 50, height: 220)
        #expect(camera.originBeat == 0 && camera.originTrack == 0)
    }

    @Test func resizingAndDeletingTracksClampBothSidesToTheSameLastRow() {
        var camera = ArrangeCamera(originTrack: 99)
        camera.constrain(trackCount: 10, height: 128)
        #expect(camera.originTrack == 8)
        camera.constrain(trackCount: 10, height: 192)
        #expect(camera.originTrack == 7)
        camera.constrain(trackCount: 2, height: 192)
        #expect(camera.originTrack == 0)
        #expect(camera.rows(trackCount: 2, height: 192).visible == 0..<2)
        camera.constrain(trackCount: 0, height: 192)
        #expect(camera.rows(trackCount: 0, height: 192).visible.isEmpty)
    }

    @Test func keyboardSelectionRevealsAnOffscreenTrackWithoutChangingTime() {
        var camera = ArrangeCamera(originBeat: 12)
        camera.reveal(track: 30, trackCount: 100, height: 128)
        #expect(camera.originTrack == 29)
        #expect(camera.rows(trackCount: 100, height: 128).visible.contains(30))
        camera.reveal(track: 3, trackCount: 100, height: 128)
        #expect(camera.originTrack == 3 && camera.originBeat == 12)
    }

    @Test func headersEnumerateOnlyVisibleTracksWithoutALimitOnTheProject() {
        let camera = ArrangeCamera(originTrack: 567_123.75)
        let rows = camera.rows(trackCount: 1_000_000, height: 201)
        #expect(rows.visible == 567_123..<567_127)
        #expect(rows.index(at: 0) == 567_123)
        #expect(rows.rect(at: 567_123, width: 160).minY == -48)
    }

    @Test func headerRowsMatchRustTimelineLanesAfterScrollZoomAndResize() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-arrange-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        // Track count and viewport sizes belong to this regression, not a cap.
        for index in 0..<12 { await controller.addTrack(name: "Track \(index)") }
        let snapshot = try #require(controller.snapshot)
        var camera = ArrangeCamera(originTrack: 2.25)
        for height in [128.0, 211.0, 480.0] {
            camera.scroll(x: -18, y: -13, trackCount: snapshot.trackCount, height: height)
            camera.zoom(
                factor: 1.25, horizontal: false, vertical: true, anchor: CGPoint(x: 0, y: 40),
                trackCount: snapshot.trackCount, height: height)
            let rows = camera.rows(trackCount: snapshot.trackCount, height: height)
            let lanes = snapshot.visibleElements(
                originTick: 0, widthTicks: 705_600 * 8,
                originTrack: camera.originTrack, heightTracks: height / camera.pixelsPerTrack,
                pixelsPerTick: camera.pixelsPerBeat / 705_600, pixelsPerTrack: camera.pixelsPerTrack
            )
            .elements.filter { $0.role == .track }
            #expect(lanes.count == rows.visible.count)
            for (index, lane) in zip(rows.visible, lanes) {
                let header = rows.rect(at: index, width: 160)
                #expect(lane.id.track == snapshot.trackID(at: index))
                #expect(abs(Double(lane.y) - Double(header.minY)) < 0.0001)
                #expect(abs(Double(lane.height) - Double(header.height)) < 0.0001)
                let point = max(0, Double(header.minY)) + 0.01
                #expect(rows.index(at: point) == index)
            }
        }
        // Offscreen AppKit layout: no window, display link, or GPU is started.
        let pane = ArrangePaneView(frame: NSRect(x: 0, y: 0, width: 600, height: 128))
        pane.layout()
        pane.setSnapshot(snapshot, selected: snapshot.trackID(at: 0))
        #expect(pane.window == nil && !pane.timeline.rendererAvailable)
        #expect(pane.headers.frame.minY == pane.timeline.frame.minY + 24)
        #expect(pane.headers.bounds.height == pane.timeline.bounds.height - 24)
        #expect(pane.timeline.bounds.height == 128)
        pane.headers.onScroll?(-18, -96)
        #expect(pane.headers.camera == pane.timeline.camera)
        #expect(pane.timeline.camera.originTrack == 1.5)
        pane.timeline.onScroll?(0, -16)
        #expect(pane.headers.camera == pane.timeline.camera)
        pane.timeline.onZoom?(1.25, .option, CGPoint(x: 100, y: 40))
        #expect(pane.headers.camera == pane.timeline.camera)
        #expect(pane.headers.camera.pixelsPerTrack == 80)
        pane.headers.onZoom?(1.2, .option, CGPoint(x: 0, y: 32))
        #expect(pane.headers.camera == pane.timeline.camera)
        #expect(pane.headers.camera.pixelsPerTrack == 96)
        pane.setFrameSize(NSSize(width: 750, height: 320))
        pane.layout()
        #expect(pane.headers.bounds.height == pane.timeline.bounds.height - 24)
        #expect(pane.timeline.bounds.height == 320)
        #expect(pane.headers.camera == pane.timeline.camera)
        #expect(pane.headers.bounds.width == 160)
        #expect(pane.timeline.frame == pane.bounds)
        #expect(pane.timeline.renderInput.header?.width == 160)
        pane.resizeHeader(to: 200)
        #expect(pane.headers.bounds.width == 200)
        #expect(pane.timeline.frame == pane.bounds)
        #expect(ArrangeFrame(input: pane.timeline.renderInput).timelineOrigin == 201)
        pane.headers.moveToEndOfDocument(nil)
        #expect(pane.headers.camera == pane.timeline.camera)
        #expect(pane.headers.camera.rows(trackCount: snapshot.trackCount, height: 320).visible.contains(11))
        pane.headers.moveToBeginningOfDocument(nil)
        #expect(pane.timeline.camera.originTrack == 0)
        await controller.close()
    }

    @Test func aDelayedFrameKeepsBothSidesTogetherDuringRapidInput() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-frame-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        // This project's size and event count are regression parameters only.
        for index in 0..<20 { await controller.addTrack(name: "Track \(index)") }
        let snapshot = try #require(controller.snapshot)
        let input = CanvasInput(
            snapshot: snapshot, selected: snapshot.trackID(at: 2), camera: ArrangeCamera(originTrack: 1.25),
            width: 640, height: 211, scale: 2, header: ArrangeHeaderInput(width: 160, palette: ArrangeHeaderPalette()))
        let oldFrame = ArrangeFrame(input: input)
        var camera = input.camera
        camera.scroll(x: 0, y: -16, trackCount: snapshot.trackCount, height: input.height)
        // A legal interleaving in the former two-surface design: AppKit has
        // drawn the latest camera while Metal still displays the older one.
        let liveHeaderY = camera.rows(trackCount: snapshot.trackCount, height: input.height).rect(at: 2, width: 160)
            .minY
        let oldLane = try #require(
            oldFrame.elements.first { $0.role == .track && $0.id.track == snapshot.trackID(at: 2) })
        #expect(abs(Double(oldLane.y) - Double(liveHeaderY)) == 16)
        var displayed = oldFrame
        for event in 0..<240 {
            camera.scroll(x: -3, y: event % 48 < 24 ? -11 : 11, trackCount: snapshot.trackCount, height: input.height)
            if event % 17 == 0 {
                camera.zoom(
                    factor: event % 34 == 0 ? 1.2 : 1 / 1.2, horizontal: true, vertical: true,
                    anchor: CGPoint(x: 90, y: 67), trackCount: snapshot.trackCount, height: input.height)
            }
            // Coalesce events and hold old frames, as with a busy GPU. Input
            // never changes either half of an already built/presented frame.
            if event % 5 == 0 {
                displayed = ArrangeFrame(
                    input: CanvasInput(
                        snapshot: snapshot, selected: input.selected, camera: camera,
                        width: input.width, height: input.height, scale: input.scale,
                        header: ArrangeHeaderInput(width: event % 2 == 0 ? 160 : 200, palette: ArrangeHeaderPalette())))
            }
            let lanes = displayed.elements.filter { $0.role == .track }
            #expect(displayed.headers.count == lanes.count)
            for (header, lane) in zip(displayed.headers, lanes) {
                #expect(header.id == lane.id.track && header.y == lane.y && header.height == lane.height)
            }
        }
        #expect(oldFrame.camera.originTrack == 1.25 && oldFrame.timelineOrigin == 161)
        await controller.close()
    }

    @Test func cachedGeometryFollowsDocumentAndViewportRevisions() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(
            "echode-geometry-cache-\(UUID()).echode")
        defer { removeTestFile(root) }
        let controller = ProjectController()
        await controller.open(root, creating: true)
        await controller.addTrack(name: "first")
        let camera = ArrangeCamera(pixelsPerBeat: 100, pixelsPerTrack: 40)
        var input = CanvasInput(
            snapshot: controller.snapshot, selected: nil, camera: camera,
            width: 320, height: 200, scale: 1)
        var cache = CanvasGeometryCache()
        let first = try #require(cache.frames(input: input, camera: camera, revision: 1).arranged)
        let again = try #require(cache.frames(input: input, camera: camera, revision: 1).arranged)
        #expect(cache.builds == 1)
        #expect(first.elements.count == again.elements.count)
        #expect(first.elements.filter { $0.role == .track }.count == 1)
        await controller.addTrack(name: "second")
        input = CanvasInput(
            snapshot: controller.snapshot, selected: nil, camera: camera,
            width: 160, height: 200, scale: 2)
        let changed = try #require(cache.frames(input: input, camera: camera, revision: 2).arranged)
        #expect(cache.builds == 2)
        #expect(changed.elements.filter { $0.role == .track }.count == 2)
        #expect(changed.timelineWidth == 160 && changed.input.scale == 2)
        var moved = camera
        moved.originBeat = 2
        let shifted = try #require(cache.frames(input: input, camera: moved, revision: 2).arranged)
        #expect(cache.builds == 3 && shifted.camera.originBeat == 2)
        #expect(first.input.width == 320 && first.elements.filter { $0.role == .track }.count == 1)
        await controller.close()
    }

    @Test func labelsShapeUnicodeAndTruncateAtTheAvailableWidth() throws {
        for scale in [1.0, 2.0] {
            let empty = try #require(ArrangeLabelRaster(name: "", width: 40, scale: scale))
            #expect(empty.pixels.allSatisfy { $0 == 0 })
            for name in ["Track", "ピアノと弦楽器 🎹", "A very long track name that needs truncation"] {
                let raster = try #require(ArrangeLabelRaster(name: name, width: 40, scale: scale))
                #expect(raster.width == Int(40 * scale) && raster.height == Int(20 * scale))
                #expect(raster.pixels.contains { $0 > 0 })
                #expect(raster.pixels.count == raster.width * raster.height)
            }
        }
    }

    @Test func contractMismatchedDrawableKeepsTopLeftPixels() async throws {
        guard let device = MTLCreateSystemDefaultDevice(), let queue = device.makeCommandQueue() else {
            throw CocoaError(.featureUnsupported)
        }
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-mismatch-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        for index in 0..<3 { await controller.addTrack(name: "Track \(index)") }
        let snapshot = try #require(controller.snapshot)
        let selected = try #require(snapshot.trackID(at: 0))
        // Sizes and the integer scale are scoped to this regression: they keep
        // the reference and stale attachments on the same pixel grid.
        let input = CanvasInput(
            snapshot: snapshot, selected: selected,
            camera: ArrangeCamera(originBeat: 2, pixelsPerBeat: 72, pixelsPerTrack: 64),
            width: 320, height: 160, scale: 2,
            header: ArrangeHeaderInput(width: 120, palette: ArrangeHeaderPalette()))
        // 160pt with 64pt rows leaves the last row partially visible; its
        // mute/solo labels cross the bottom edge, so a stale taller drawable
        // must clip them at the input height.
        let frame = ArrangeFrame(input: input)
        let painter = try ArrangePainter(device: device)
        let reference = try renderOffscreen(frame, painter: painter, device: device, queue: queue)
        #expect(reference.width == 640 && reference.height == 320)
        let background = CanvasReadback.Pixel.canvasBackground
        let painted = await reference.scan { _, _, pixel in pixel != background }
        #expect(painted.count > 1_000)
        for size in [CGSize(width: 700, height: 380), CGSize(width: 600, height: 280)] {
            let image = try renderOffscreen(frame, painter: painter, device: device, queue: queue, size: size)
            #expect(image.width == Int(size.width) && image.height == Int(size.height))
            let differing = await image.scan { x, y, pixel in
                x < reference.width && y < reference.height && pixel != reference.pixel(x: x, y: y)
            }
            #expect(differing.count == 0, "stale \(size) differs in \(differing.count) overlapping pixels")
            if image.width > reference.width || image.height > reference.height {
                let margin = await image.scan { x, y, pixel in
                    (x >= reference.width || y >= reference.height) && pixel != background
                }
                #expect(margin.count == 0, "stale \(size) margin differs at \(margin.first ?? "none")")
            }
        }
        let piano = PianoFrame(
            input: input,
            piano: PianoRenderInput(
                track: selected, focus: nil, camera: PianoCamera(),
                key: DisplayKey(), preview: nil))
        let pianoReference = try renderOffscreen(piano, painter: painter, device: device, queue: queue)
        let pianoStale = try renderOffscreen(
            piano, painter: painter, device: device, queue: queue,
            size: CGSize(width: 700, height: 380))
        let pianoDiffering = await pianoStale.scan { x, y, pixel in
            x < pianoReference.width && y < pianoReference.height && pixel != pianoReference.pixel(x: x, y: y)
        }
        #expect(pianoDiffering.count == 0)
        let pianoMargin = await pianoStale.scan { x, y, pixel in
            (x >= pianoReference.width || y >= pianoReference.height) && pixel != background
        }
        #expect(pianoMargin.count == 0, "piano margin differs at \(pianoMargin.first ?? "none")")
        let matching = CanvasDrawablePolicy.resolve(input: input, drawableSize: input.drawableSize)
        #expect(!matching.mismatched && matching.draws && matching.consumesRevision)
        let stale = CanvasDrawablePolicy.resolve(input: input, drawableSize: CGSize(width: 600, height: 280))
        #expect(stale.mismatched && stale.draws && !stale.consumesRevision)
        #expect(
            !canvasLinkPaused(
                ready: true, continuous: false, metersActive: false, playing: false,
                revisionDrawn: true, staleDrawable: stale.mismatched))
        await controller.close()
    }

    @Test func anAudioClipDrawsItsPeaksInsideTheLane() async throws {
        guard let device = MTLCreateSystemDefaultDevice(), let queue = device.makeCommandQueue() else {
            throw CocoaError(.featureUnsupported)
        }
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-wave-\(UUID())")
        defer { removeTestFile(root) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let wav = root.appendingPathComponent("tone.wav")
        try alternatingWav(rate: 48_000, frames: 4_800).write(to: wav)
        let url = root.appendingPathComponent("Song.echode")
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "audio")
        #expect(await controller.importAudio(wav))
        let snapshot = try #require(controller.snapshot)
        let camera = ArrangeCamera(pixelsPerBeat: 400, pixelsPerTrack: 80)
        let frame = ArrangeFrame(
            input: CanvasInput(snapshot: snapshot, selected: nil, camera: camera, width: 240, height: 160, scale: 1))
        let clip = try #require(frame.elements.first { $0.role == .clip })
        #expect(!frame.waveforms.isEmpty)
        let painter = try ArrangePainter(device: device)
        let image = try renderOffscreen(frame, painter: painter, device: device, queue: queue)
        let top = Int(clip.y) + 4
        let loud = image.pixel(x: Int(clip.x) + 6, y: top)
        let quiet = image.pixel(x: Int(clip.x + clip.width) - 6, y: top)
        #expect(loud.r > 160 && loud.g > 180)
        #expect(quiet.r < 80 && quiet.b > quiet.r)
        await controller.close()
    }
}

private func alternatingWav(rate: UInt32, frames: Int) -> Data {
    var data = Data()
    data.append(contentsOf: [0x52, 0x49, 0x46, 0x46])
    let dataBytes = UInt32(frames * 2)
    data.append(contentsOf: withUnsafeBytes(of: (36 + dataBytes).littleEndian) { Array($0) })
    data.append(contentsOf: [0x57, 0x41, 0x56, 0x45, 0x66, 0x6d, 0x74, 0x20])
    data.append(contentsOf: withUnsafeBytes(of: UInt32(16).littleEndian) { Array($0) })
    data.append(contentsOf: withUnsafeBytes(of: UInt16(1).littleEndian) { Array($0) })
    data.append(contentsOf: withUnsafeBytes(of: UInt16(1).littleEndian) { Array($0) })
    data.append(contentsOf: withUnsafeBytes(of: rate.littleEndian) { Array($0) })
    data.append(contentsOf: withUnsafeBytes(of: (rate * 2).littleEndian) { Array($0) })
    data.append(contentsOf: withUnsafeBytes(of: UInt16(2).littleEndian) { Array($0) })
    data.append(contentsOf: withUnsafeBytes(of: UInt16(16).littleEndian) { Array($0) })
    data.append(contentsOf: [0x64, 0x61, 0x74, 0x61])
    data.append(contentsOf: withUnsafeBytes(of: dataBytes.littleEndian) { Array($0) })
    for index in 0..<frames {
        let sample: Int16 = index < frames / 2 ? (index.isMultiple(of: 2) ? 32767 : -32767) : 0
        data.append(contentsOf: withUnsafeBytes(of: sample.littleEndian) { Array($0) })
    }
    return data
}
