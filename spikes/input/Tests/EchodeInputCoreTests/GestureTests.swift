import Testing

@testable import EchodeInputCore

struct GestureTests {
    /// XCTest's `accuracy:` comparison, which Swift Testing does not have.
    private func close(_ actual: Double, _ expected: Double) -> Bool {
        abs(actual - expected) <= 0.0001
    }

    @Test func trackpadSpanUsesThePadSurfaceNotTheView() {
        let span = fingerSpan(
            first: TrackpadTouch(x: 0.2, y: 0.5, deviceWidth: 300, deviceHeight: 200),
            second: TrackpadTouch(x: 0.5, y: 0.5, deviceWidth: 300, deviceHeight: 200)
        )
        #expect(close(span.x, 90))
        #expect(close(span.y, 0))
        var gesture = PinchGesture(span: span, zoom: Zoom(x: 1, y: 1), mode: .directional)
        let locked = gesture.update(span: FingerSpan(x: 180, y: 0))
        #expect(gesture.axis == .horizontal)
        #expect(close(locked.x, 1))
        let zoom = gesture.update(span: FingerSpan(x: 270, y: 0))
        #expect(close(zoom.x, 1.5))
        #expect(close(zoom.y, 1))
    }

    @Test func aNormalPinchScalesBothAxesTogether() {
        var gesture = PinchGesture(
            span: FingerSpan(x: 100, y: 100),
            zoom: Zoom(x: 1, y: 2),
            mode: .uniform
        )
        let zoom = gesture.update(span: FingerSpan(x: 150, y: 150))
        #expect(close(zoom.x, 1.5))
        #expect(close(zoom.y, 3))
        #expect(gesture.axis == .uniform)
    }

    @Test func directionalModeLocksAClearlyHorizontalPinch() {
        var gesture = PinchGesture(
            span: FingerSpan(x: 100, y: 80),
            zoom: Zoom(x: 1, y: 2),
            mode: .directional
        )
        let locked = gesture.update(span: FingerSpan(x: 150, y: 80))
        #expect(gesture.axis == .horizontal)
        #expect(close(locked.x, 1))
        #expect(close(locked.y, 2))
        let zoom = gesture.update(span: FingerSpan(x: 225, y: 80))
        #expect(close(zoom.x, 1.5))
        #expect(close(zoom.y, 2))
    }

    @Test func theLockedAxisDoesNotFollowTheOtherAxis() {
        var gesture = PinchGesture(
            span: FingerSpan(x: 100, y: 80),
            zoom: Zoom(x: 1, y: 1),
            mode: .directional
        )
        let opened = gesture.update(span: FingerSpan(x: 140, y: 86))
        #expect(gesture.axis == .horizontal)
        #expect(gesture.axisLocked)
        #expect(close(opened.x, 1))
        let wobble = gesture.update(span: FingerSpan(x: 210, y: 140))
        #expect(gesture.axis == .horizontal)
        #expect(close(wobble.y, 1))
        #expect(close(wobble.x, 1.5))
    }

    @Test func directionalModeLocksAClearlyVerticalPinch() {
        var gesture = PinchGesture(
            span: FingerSpan(x: 90, y: 40),
            zoom: Zoom(x: 1, y: 1),
            mode: .directional
        )
        let locked = gesture.update(span: FingerSpan(x: 90, y: 90))
        #expect(gesture.axis == .vertical)
        #expect(close(locked.x, 1))
        #expect(close(locked.y, 1))
        let zoom = gesture.update(span: FingerSpan(x: 90, y: 180))
        #expect(close(zoom.x, 1))
        #expect(close(zoom.y, 2))
    }

    @Test func directionalModeDoesNotMoveBeforeTheAxisIsClear() {
        var gesture = PinchGesture(
            span: FingerSpan(x: 100, y: 80),
            zoom: Zoom(x: 1, y: 2),
            mode: .directional
        )
        let early = gesture.update(span: FingerSpan(x: 108, y: 86))
        #expect(!gesture.axisLocked)
        #expect(close(early.x, 1))
        #expect(close(early.y, 2))
    }

    @Test func fingersCrossingDoesNotThrowTheZoom() {
        var gesture = PinchGesture(
            span: FingerSpan(x: 100, y: 80),
            zoom: Zoom(x: 1, y: 1),
            mode: .uniform
        )
        let opened = gesture.update(span: FingerSpan(x: 140, y: 80))
        let crossed = gesture.update(span: FingerSpan(x: 140, y: 4))
        let restored = gesture.update(span: FingerSpan(x: 140, y: 80))
        #expect(close(crossed.x, opened.x))
        #expect(close(crossed.y, opened.y))
        #expect(close(restored.x, opened.x))
        #expect(close(restored.y, opened.y))
    }

    @Test func pinchingInwardStillZoomsOut() {
        var gesture = PinchGesture(
            span: FingerSpan(x: 100, y: 100),
            zoom: Zoom(x: 2, y: 2),
            mode: .uniform
        )
        let zoom = gesture.update(span: FingerSpan(x: 50, y: 50))
        #expect(close(zoom.x, 1))
        #expect(close(zoom.y, 1))
    }

    @Test func zoomStaysInsideItsRange() {
        var gesture = PinchGesture(
            span: FingerSpan(x: 20, y: 20),
            zoom: Zoom(x: 7, y: 1),
            mode: .uniform
        )
        let zoom = gesture.update(span: FingerSpan(x: 200, y: 200))
        #expect(close(zoom.x, maximumZoom))
        #expect(close(zoom.y, maximumZoom))
    }

    @Test func gridCrossing() {
        #expect(crossedGridLine(from: 9, to: 11, spacing: 10))
        #expect(!crossedGridLine(from: 11, to: 19, spacing: 10))
        #expect(!crossedGridLine(from: 10, to: 10, spacing: 0))
    }

    @Test func aChordSurvivesReleasingOneKey() {
        var keyboard = ComputerKeyboard()
        keyboard.press("a")
        keyboard.press("d")
        #expect(keyboard.heldNotes == [48, 52])
        keyboard.release("a")
        #expect(keyboard.heldNotes == [52])
        keyboard.release("D")
        #expect(keyboard.heldNotes.isEmpty)
    }

    @Test func pressureRequiresAHeldChordAndIsClamped() {
        #expect(chordPressure(notes: [], pressure: 0.4) == nil)
        let expression = chordPressure(notes: [52, 48], pressure: 1.8)
        #expect(expression?.notes == [48, 52])
        #expect(close(expression?.pressure ?? -1, 1))
        #expect(noteName(48) == "C3")
        #expect(noteName(52) == "E3")
    }
}
