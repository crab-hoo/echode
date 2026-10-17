// swift-tools-version: 6.0
import PackageDescription

let strict: [SwiftSetting] = [.unsafeFlags(["-strict-concurrency=complete", "-warnings-as-errors", "-target-cpu", "apple-m4", "-Xcc", "-mcpu=apple-m4"])]
let package = Package(
    name: "EchodeApp",
    platforms: [.macOS("27.0")],
    products: [.executable(name: "Echode", targets: ["EchodeApp"]), .executable(name: "EchodeAppProbe", targets: ["EchodeAppProbe"]),
               .executable(name: "EchodePlayheadProbe", targets: ["EchodePlayheadProbe"])],
    dependencies: [.package(path: "../EchodeBridge")],
    targets: [
        .target(name: "EchodeAppCore", dependencies: [.product(name: "EchodeBridge", package: "EchodeBridge")],
                resources: [.process("Resources")], swiftSettings: strict),
        .executableTarget(name: "EchodeApp", dependencies: ["EchodeAppCore"], swiftSettings: strict),
        .executableTarget(name: "EchodeAppProbe", dependencies: ["EchodeAppCore"], swiftSettings: strict),
        .target(name: "EchodePlayheadChecks", dependencies: ["EchodeAppCore"], path: "Tests/EchodePlayheadChecks", swiftSettings: strict),
        .executableTarget(name: "EchodePlayheadProbe", dependencies: ["EchodePlayheadChecks"], swiftSettings: strict),
        .testTarget(name: "EchodeAppTests", dependencies: ["EchodeAppCore"], swiftSettings: strict),
    ]
)
