import XCTest
@testable import EchodeInputCore

final class GestureTests: XCTestCase {
    func testTrackpadSpanUsesThePadSurfaceNotTheView() {
        let span = fingerSpan(
            first: TrackpadTouch(x: 0.2, y: 0.5, deviceWidth: 300, deviceHeight: 200),
            second: TrackpadTouch(x: 0.5, y: 0.5, deviceWidth: 300, deviceHeight: 200)
        )
        XCTAssertEqual(span.x, 90, accuracy: 0.0001)
        XCTAssertEqual(span.y, 0, accuracy: 0.0001)
        var gesture = PinchGesture(span: span, zoom: Zoom(x: 1, y: 1), mode: .directional)
        let locked = gesture.update(span: FingerSpan(x: 180, y: 0))
        XCTAssertEqual(gesture.axis, .horizontal)
        XCTAssertEqual(locked.x, 1, accuracy: 0.0001)
        let zoom = gesture.update(span: FingerSpan(x: 270, y: 0))
        XCTAssertEqual(zoom.x, 1.5, accuracy: 0.0001)
        XCTAssertEqual(zoom.y, 1, accuracy: 0.0001)
    }

    func testANormalPinchScalesBothAxesTogether() {
        var gesture = PinchGesture(
            span: FingerSpan(x: 100, y: 100),
            zoom: Zoom(x: 1, y: 2),
            mode: .uniform
        )
        let zoom = gesture.update(span: FingerSpan(x: 150, y: 150))
        XCTAssertEqual(zoom.x, 1.5, accuracy: 0.0001)
        XCTAssertEqual(zoom.y, 3, accuracy: 0.0001)
        XCTAssertEqual(gesture.axis, .uniform)
    }

    func testDirectionalModeLocksAClearlyHorizontalPinch() {
        var gesture = PinchGesture(
            span: FingerSpan(x: 100, y: 80),
            zoom: Zoom(x: 1, y: 2),
            mode: .directional
        )
        let locked = gesture.update(span: FingerSpan(x: 150, y: 80))
        XCTAssertEqual(gesture.axis, .horizontal)
        XCTAssertEqual(locked.x, 1, accuracy: 0.0001)
        XCTAssertEqual(locked.y, 2, accuracy: 0.0001)
        let zoom = gesture.update(span: FingerSpan(x: 225, y: 80))
        XCTAssertEqual(zoom.x, 1.5, accuracy: 0.0001)
        XCTAssertEqual(zoom.y, 2, accuracy: 0.0001)
    }

    func testTheLockedAxisDoesNotFollowTheOtherAxis() {
        var gesture = PinchGesture(
            span: FingerSpan(x: 100, y: 80),
            zoom: Zoom(x: 1, y: 1),
            mode: .directional
        )
        let opened = gesture.update(span: FingerSpan(x: 140, y: 86))
        XCTAssertEqual(gesture.axis, .horizontal)
        XCTAssertTrue(gesture.axisLocked)
        XCTAssertEqual(opened.x, 1, accuracy: 0.0001)
        let wobble = gesture.update(span: FingerSpan(x: 210, y: 140))
        XCTAssertEqual(gesture.axis, .horizontal)
        XCTAssertEqual(wobble.y, 1, accuracy: 0.0001)
        XCTAssertEqual(wobble.x, 1.5, accuracy: 0.0001)
    }

    func testDirectionalModeLocksAClearlyVerticalPinch() {
        var gesture = PinchGesture(
            span: FingerSpan(x: 90, y: 40),
            zoom: Zoom(x: 1, y: 1),
            mode: .directional
        )
        let locked = gesture.update(span: FingerSpan(x: 90, y: 90))
        XCTAssertEqual(gesture.axis, .vertical)
        XCTAssertEqual(locked.x, 1, accuracy: 0.0001)
        XCTAssertEqual(locked.y, 1, accuracy: 0.0001)
        let zoom = gesture.update(span: FingerSpan(x: 90, y: 180))
        XCTAssertEqual(zoom.x, 1, accuracy: 0.0001)
        XCTAssertEqual(zoom.y, 2, accuracy: 0.0001)
    }

    func testDirectionalModeDoesNotMoveBeforeTheAxisIsClear() {
        var gesture = PinchGesture(
            span: FingerSpan(x: 100, y: 80),
            zoom: Zoom(x: 1, y: 2),
            mode: .directional
        )
        let early = gesture.update(span: FingerSpan(x: 108, y: 86))
        XCTAssertFalse(gesture.axisLocked)
        XCTAssertEqual(early.x, 1, accuracy: 0.0001)
        XCTAssertEqual(early.y, 2, accuracy: 0.0001)
    }

    func testFingersCrossingDoesNotThrowTheZoom() {
        var gesture = PinchGesture(
            span: FingerSpan(x: 100, y: 80),
            zoom: Zoom(x: 1, y: 1),
            mode: .uniform
        )
        let opened = gesture.update(span: FingerSpan(x: 140, y: 80))
        let crossed = gesture.update(span: FingerSpan(x: 140, y: 4))
        let restored = gesture.update(span: FingerSpan(x: 140, y: 80))
        XCTAssertEqual(crossed.x, opened.x, accuracy: 0.0001)
        XCTAssertEqual(crossed.y, opened.y, accuracy: 0.0001)
        XCTAssertEqual(restored.x, opened.x, accuracy: 0.0001)
        XCTAssertEqual(restored.y, opened.y, accuracy: 0.0001)
    }

    func testPinchingInwardStillZoomsOut() {
        var gesture = PinchGesture(
            span: FingerSpan(x: 100, y: 100),
            zoom: Zoom(x: 2, y: 2),
            mode: .uniform
        )
        let zoom = gesture.update(span: FingerSpan(x: 50, y: 50))
        XCTAssertEqual(zoom.x, 1, accuracy: 0.0001)
        XCTAssertEqual(zoom.y, 1, accuracy: 0.0001)
    }

    func testZoomStaysInsideItsRange() {
        var gesture = PinchGesture(
            span: FingerSpan(x: 20, y: 20),
            zoom: Zoom(x: 7, y: 1),
            mode: .uniform
        )
        let zoom = gesture.update(span: FingerSpan(x: 200, y: 200))
        XCTAssertEqual(zoom.x, maximumZoom, accuracy: 0.0001)
        XCTAssertEqual(zoom.y, maximumZoom, accuracy: 0.0001)
    }

    func testGridCrossing() {
        XCTAssertTrue(crossedGridLine(from: 9, to: 11, spacing: 10))
        XCTAssertFalse(crossedGridLine(from: 11, to: 19, spacing: 10))
        XCTAssertFalse(crossedGridLine(from: 10, to: 10, spacing: 0))
    }

    func testAChordSurvivesReleasingOneKey() {
        var keyboard = ComputerKeyboard()
        keyboard.press("a")
        keyboard.press("d")
        XCTAssertEqual(keyboard.heldNotes, [48, 52])
        keyboard.release("a")
        XCTAssertEqual(keyboard.heldNotes, [52])
        keyboard.release("D")
        XCTAssertTrue(keyboard.heldNotes.isEmpty)
    }

    func testPressureRequiresAHeldChordAndIsClamped() {
        XCTAssertNil(chordPressure(notes: [], pressure: 0.4))
        let expression = chordPressure(notes: [52, 48], pressure: 1.8)
        XCTAssertEqual(expression?.notes, [48, 52])
        XCTAssertEqual(expression?.pressure ?? -1, 1, accuracy: 0.0001)
        XCTAssertEqual(noteName(48), "C3")
        XCTAssertEqual(noteName(52), "E3")
    }
}
