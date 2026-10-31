@testable import EchodeAppCore

struct PixelScan: Sendable {
    var count = 0
    /// Names the first match for failure messages.
    var first: String?
}

extension CanvasReadback {
    /// Counts the matching pixels of the whole image.
    /// A full scan takes hundreds of milliseconds under a sanitizer. Off the main actor it does not
    /// starve the tests that poll it against a deadline (docs/debugging.md).
    func scan(where matches: @escaping @Sendable (_ x: Int, _ y: Int, _ pixel: Pixel) -> Bool) async -> PixelScan {
        await Task.detached { [self] in
            var result = PixelScan()
            for y in 0..<height {
                for x in 0..<width where matches(x, y, pixel(x: x, y: y)) {
                    result.count += 1
                    result.first = result.first ?? "\(x),\(y)"
                }
            }
            return result
        }.value
    }
}
