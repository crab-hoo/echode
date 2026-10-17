// swift-tools-version: 6.0
import PackageDescription

let m4: [SwiftSetting] = [.unsafeFlags(["-target-cpu", "apple-m4", "-Xcc", "-mcpu=apple-m4"])]

let package = Package(
    name: "EchodeInput",
    platforms: [.macOS("27.0")],
    products: [
        .library(name: "EchodeInputCore", targets: ["EchodeInputCore"]),
        .executable(name: "EchodeInput", targets: ["EchodeInput"]),
    ],
    targets: [
        .target(name: "EchodeInputCore", path: "Sources/EchodeInputCore", swiftSettings: m4),
        .executableTarget(
            name: "EchodeInput",
            dependencies: ["EchodeInputCore"],
            path: "Sources/EchodeInput",
            swiftSettings: m4,
            linkerSettings: [
                .linkedFramework("AppKit")
            ]
        ),
        .testTarget(
            name: "EchodeInputCoreTests",
            dependencies: ["EchodeInputCore"],
            path: "Tests/EchodeInputCoreTests",
            swiftSettings: m4
        ),
    ]
)
