import AppKit
import CoreGraphics
import Foundation

/// A short real-display measurement, invoked only by make spike-app.
@MainActor
public func runArrangeProbe(reportPath: String) {
    let app = NSApplication.shared
    app.setActivationPolicy(.regular)
    let window = NSWindow(
        contentRect: NSRect(x: 100, y: 100, width: 900, height: 650),
        styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
    window.title = "Echode canvas probe"
    let content = NSView(frame: NSRect(x: 0, y: 0, width: 900, height: 650))
    let pane = ArrangePaneView(frame: NSRect(x: 0, y: 330, width: 900, height: 320))
    let view = pane.timeline
    let piano = PianoRollView(frame: NSRect(x: 0, y: 0, width: 900, height: 330))
    content.addSubview(pane)
    content.addSubview(piano)
    window.contentView = content
    window.makeKeyAndOrderFront(nil)
    app.activate()
    let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-arrange-probe-\(UUID()).echode")
    Task { @MainActor in
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "Probe")
        await controller.insertNote(pitch: 60, beat: 0, length: 8, documentID: controller.documentID!)
        // This project size belongs to the explicit display regression only.
        for index in 1..<20 { await controller.addTrack(name: "Track \(index)") }
        guard let snapshot = controller.snapshot else { exit(1) }
        guard let track = snapshot.trackID(at: 0) else { exit(1) }
        pane.setSnapshot(snapshot, selected: track)
        piano.grid.setSnapshot(snapshot, track: track, focus: nil, key: DisplayKey(), busy: false)
        DispatchQueue.main.asyncAfter(deadline: .now() + 1) {
            guard view.rendererAvailable, piano.grid.rendererAvailable else {
                print("Metal renderer is unavailable")
                exit(1)
            }
            view.beginMeasurement()
            piano.grid.beginMeasurement()
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
                view.markMainBusy(true)
                piano.grid.markMainBusy(true)
                let deadline = Date().addingTimeInterval(0.5)
                // Deliberate MainActor load in this explicit probe only.
                while Date() < deadline {}
                view.markMainBusy(false)
                piano.grid.markMainBusy(false)
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
                    let measured = view.measurement
                    let children = view.accessibilityChildren() as? [NSAccessibilityElement] ?? []
                    let again = view.accessibilityChildren() as? [NSAccessibilityElement] ?? []
                    let accessible =
                        children.count >= 3 && again.count == children.count
                        && zip(children, again).allSatisfy { $0 === $1 }
                        && children.allSatisfy { $0.isAccessibilityElement() }
                    let headerChildren = pane.headers.accessibilityChildren() as? [NSAccessibilityElement] ?? []
                    let headerAgain = pane.headers.accessibilityChildren() as? [NSAccessibilityElement] ?? []
                    let headersAccessible =
                        !headerChildren.isEmpty && headerChildren.count == headerAgain.count
                        && zip(headerChildren, headerAgain).allSatisfy { $0 === $1 }
                        && headerChildren.allSatisfy { $0.isAccessibilityElement() }
                    let pianoMeasured = piano.grid.measurement
                    let pianoChildren = piano.grid.accessibilityChildren() as? [NSAccessibilityElement] ?? []
                    let pianoAgain = piano.grid.accessibilityChildren() as? [NSAccessibilityElement] ?? []
                    let pianoAccessible =
                        pianoChildren.count == 1 && pianoAgain.count == 1 && pianoChildren[0] === pianoAgain[0]
                        && pianoChildren[0].isAccessibilityElement()
                    let aligned =
                        pane.headers.camera == view.camera
                        && pane.headers.frame.minY == view.frame.minY + view.renderInput.arrangeTop
                        && pane.headers.bounds.height == view.renderInput.arrangeHeight
                        && measured.headerFrames == measured.frames && measured.maxHeaderLaneError == 0
                    let passed =
                        aligned && headersAccessible && measured.frames > 10
                        && measured.framesOffMain == measured.frames && measured.framesWhileMainBusy > 5 && accessible
                        && pianoMeasured.frames > 10 && pianoMeasured.framesOffMain == pianoMeasured.frames
                        && pianoMeasured.framesWhileMainBusy > 5 && pianoAccessible
                    let display = DisplayFacts(window: window)
                    let classification = spikeClassification(
                        passed: passed, display: display, arrange: measured, piano: pianoMeasured)
                    let report = SpikeAppReport(
                        arrange: measured, piano: pianoMeasured, display: display, classification: classification)
                    do {
                        let encoder = JSONEncoder()
                        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
                        try encoder.encode(report).write(to: URL(fileURLWithPath: reportPath))
                    } catch {
                        print(error)
                        exit(1)
                    }
                    print("classification: \(classification)")
                    print(
                        "display screens: \(display.screens), asleep: \(display.mainAsleep), visible: \(display.windowVisible), active space: \(display.windowOnActiveSpace), occluded: \(display.windowOccluded)"
                    )
                    print(
                        "arrange callbacks: \(measured.callbacks), wakes: \(measured.wakeCount), resumes: \(measured.resumeCount), paused: \(measured.displayLinkPaused), continuous: \(measured.continuous), revision: \(measured.revision), drawn: \(String(describing: measured.drawnRevision))"
                    )
                    print(
                        "frames: \(measured.frames), off main: \(measured.framesOffMain), while main busy: \(measured.framesWhileMainBusy), retained accessibility children: \(children.count), accessible: \(accessible), max CPU ms: \(measured.maxCPUSeconds * 1000)"
                    )
                    print(
                        "piano callbacks: \(pianoMeasured.callbacks), wakes: \(pianoMeasured.wakeCount), resumes: \(pianoMeasured.resumeCount), paused: \(pianoMeasured.displayLinkPaused), continuous: \(pianoMeasured.continuous), revision: \(pianoMeasured.revision), drawn: \(String(describing: pianoMeasured.drawnRevision))"
                    )
                    print(
                        "piano frames: \(pianoMeasured.frames), off main: \(pianoMeasured.framesOffMain), while main busy: \(pianoMeasured.framesWhileMainBusy), retained notes: \(pianoChildren.count), accessible: \(pianoAccessible)"
                    )
                    print(
                        "track headers share the timeline frame: \(aligned), retained accessible headers: \(headerChildren.count), accessible: \(headersAccessible)"
                    )
                    view.stop()
                    piano.grid.stop()
                    Task {
                        await controller.close()
                        try? FileManager.default.removeItem(at: url)
                        window.close()
                        exit(passed ? 0 : 1)
                    }
                }
            }
        }
    }
    app.run()
}

private struct SpikeAppReport: Codable {
    var arrange: ArrangeMeasurement
    var piano: ArrangeMeasurement
    var display: DisplayFacts
    var classification: String
}

private struct DisplayFacts: Codable {
    var screens: Int
    var mainAsleep: Bool
    var windowVisible: Bool
    var windowOnActiveSpace: Bool
    var windowOccluded: Bool
    var miniaturized: Bool

    @MainActor
    init(window: NSWindow) {
        screens = NSScreen.screens.count
        mainAsleep = CGDisplayIsAsleep(CGMainDisplayID()) != 0
        windowVisible = window.isVisible
        windowOnActiveSpace = window.isOnActiveSpace
        windowOccluded = !window.occlusionState.contains(.visible)
        miniaturized = window.isMiniaturized
    }
}

/// The frame threshold stays. A dark or hidden window is recorded, not treated as a pass.
private func spikeClassification(
    passed: Bool, display: DisplayFacts, arrange: ArrangeMeasurement, piano: ArrangeMeasurement
) -> String {
    if passed { return "normal" }
    let hidden =
        display.screens == 0 || display.mainAsleep || !display.windowVisible || display.windowOccluded
        || display.miniaturized
    let quiet = arrange.callbacks < 10 || piano.callbacks < 10 || arrange.resumeCount == 0 || piano.resumeCount == 0
    return hidden && quiet ? "insufficient-environment" : "failure"
}
