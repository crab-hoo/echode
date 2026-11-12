import XCTest

@testable import EchodeBridge

final class ModelLimitsTests: XCTestCase {
    func testUnitsAndBoundsMatchTheProductProtocol() {
        XCTAssertEqual(ModelLimits.ticksPerQuarter, 705_600)
        XCTAssertEqual(ModelLimits.flicksPerSecond, 705_600_000)
        XCTAssertEqual(ModelLimits.gain, 0...4)
        XCTAssertEqual(ModelLimits.pan, -1...1)
        XCTAssertEqual(ModelLimits.clipGain, 0...1_048_576)
        XCTAssertEqual(ModelLimits.fadeRateLimit, 768_000)
        XCTAssertEqual(ModelLimits.midi7, 0...127)
    }

    func testTickConversionRoundsOnceAndRejectsNonfiniteOrOverflowingBeats() {
        XCTAssertEqual(Tick.ticks(1.0 / 3), 235_200)
        XCTAssertEqual(Tick.ticks(1.0 / 256), 2_756)
        XCTAssertEqual(Tick.ticks(0.5 / 705_600), 1)
        XCTAssertEqual(Tick.ticks(-0.5 / 705_600), -1)
        for beats in [
            Double.nan, .infinity, -.infinity, Double.greatestFiniteMagnitude,
            Double(Int64.max) / 705_600, Double(Int64.min) / 705_600 * 2,
        ] {
            XCTAssertNil(Tick(beats: beats))
        }
        XCTAssertEqual(Tick(beats: -1)?.rawValue, -705_600)
        XCTAssertEqual(Tick(rawValue: .min).rawValue, .min)
        XCTAssertEqual(Tick(rawValue: .max).rawValue, .max)
        for raw: Int64 in [-7_056_001, -1, 0, 1, 235_200, 705_600, 7_056_001] {
            let tick = Tick(rawValue: raw)
            XCTAssertEqual(Tick(beats: tick.beats), tick)
        }
        XCTAssertLessThan(Tick(rawValue: -1), Tick(rawValue: 0))
    }
}
