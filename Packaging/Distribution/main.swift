import AppKit

/// Xcode compiles this only so the generated project has an application target.
/// `scripts/build-dist.sh` replaces the product with the Swift package binary.
@main
enum DistributionPlaceholder {
    static func main() {
        NSApplication.shared.run()
    }
}
