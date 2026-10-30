// swift-tools-version: 6.0
import Foundation
import PackageDescription

let packageDir = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
let rustLibraryDir = ProcessInfo.processInfo.environment["ECHODE_RUST_LIBRARY_DIR"] ?? packageDir
    .deletingLastPathComponent()
    .deletingLastPathComponent()
    .appendingPathComponent("target/debug")
    .path

let package = Package(
    name: "EchodeBridge",
    platforms: [.macOS("27.0")],
    products: [
        .library(name: "EchodeBridge", targets: ["EchodeBridge"])
    ],
    targets: [
        .target(
            name: "CEchodeFfi",
            path: "CEchodeFfi",
            publicHeadersPath: "include",
            cSettings: [.unsafeFlags(["-mcpu=apple-m4"])],
            cxxSettings: [.unsafeFlags(["-mcpu=apple-m4"])],
            linkerSettings: [
                .linkedLibrary("echode_ffi"),
                .linkedFramework("CoreAudio"),
                .linkedFramework("AudioToolbox"),
                .unsafeFlags(["-L", rustLibraryDir]),
            ]
        ),
        .target(
            name: "EchodeBridge",
            dependencies: ["CEchodeFfi"],
            path: "Sources/EchodeBridge",
            swiftSettings: [
                .unsafeFlags(["-strict-concurrency=complete", "-warnings-as-errors", "-target-cpu", "apple-m4", "-Xcc", "-mcpu=apple-m4"])
            ]
        ),
        .testTarget(
            name: "EchodeBridgeTests",
            dependencies: ["EchodeBridge"],
            path: "Tests/EchodeBridgeTests",
            resources: [.process("Fixtures")],
            swiftSettings: [
                .unsafeFlags(["-strict-concurrency=complete", "-warnings-as-errors", "-target-cpu", "apple-m4", "-Xcc", "-mcpu=apple-m4"])
            ]
        ),
    ]
)
