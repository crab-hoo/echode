// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "EchodeInput",
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "EchodeInputCore", targets: ["EchodeInputCore"]),
        .executable(name: "EchodeInput", targets: ["EchodeInput"]),
    ],
    targets: [
        .target(name: "EchodeInputCore", path: "Sources/EchodeInputCore"),
        .executableTarget(
            name: "EchodeInput",
            dependencies: ["EchodeInputCore"],
            path: "Sources/EchodeInput",
            linkerSettings: [
                .linkedFramework("AppKit")
            ]
        ),
        .testTarget(
            name: "EchodeInputCoreTests",
            dependencies: ["EchodeInputCore"],
            path: "Tests/EchodeInputCoreTests"
        ),
    ]
)
