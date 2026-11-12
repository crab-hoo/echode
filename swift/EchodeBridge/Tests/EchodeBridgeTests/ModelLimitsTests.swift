import Testing

@testable import EchodeBridge

struct ModelLimitsTests {
    @Test func unitsAndBoundsMatchTheProductProtocol() {
        #expect(ModelLimits.ticksPerQuarter == 705_600)
        #expect(ModelLimits.flicksPerSecond == 705_600_000)
        #expect(ModelLimits.gain == 0...4)
        #expect(ModelLimits.pan == -1...1)
        #expect(ModelLimits.clipGain == 0...1_048_576)
        #expect(ModelLimits.fadeRateLimit == 768_000)
        #expect(ModelLimits.midi7 == 0...127)
    }

    @Test func tickConversionRoundsOnceAndRejectsNonfiniteOrOverflowingBeats() {
        #expect(Tick.ticks(1.0 / 3) == 235_200)
        #expect(Tick.ticks(1.0 / 256) == 2_756)
        #expect(Tick.ticks(0.5 / 705_600) == 1)
        #expect(Tick.ticks(-0.5 / 705_600) == -1)
        for beats in [
            Double.nan, .infinity, -.infinity, Double.greatestFiniteMagnitude,
            Double(Int64.max) / 705_600, Double(Int64.min) / 705_600 * 2,
        ] {
            #expect(Tick(beats: beats) == nil)
        }
        #expect(Tick(beats: -1)?.rawValue == -705_600)
        #expect(Tick(rawValue: .min).rawValue == .min)
        #expect(Tick(rawValue: .max).rawValue == .max)
        for raw: Int64 in [-7_056_001, -1, 0, 1, 235_200, 705_600, 7_056_001] {
            let tick = Tick(rawValue: raw)
            #expect(Tick(beats: tick.beats) == tick)
        }
        #expect(Tick(rawValue: -1) < Tick(rawValue: 0))
    }
}
