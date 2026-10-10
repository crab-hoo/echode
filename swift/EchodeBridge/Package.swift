// swift-tools-version: 6.0
import Foundation
import PackageDescription

let packageDir = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
let rustLibraryDir = packageDir
    .deletingLastPathComponent()
    .deletingLastPathComponent()
    .appendingPathComponent("target/debug")
    .path

let package = Package(
    name: "EchodeBridge",
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "EchodeBridge", targets: ["EchodeBridge"])
    ],
    targets: [
        .target(
            name: "CEchodeFfi",
            path: "CEchodeFfi",
            publicHeadersPath: "include",
            linkerSettings: [
                .linkedLibrary("echode_ffi"),
                .unsafeFlags(["-L", rustLibraryDir]),
            ]
        ),
        .target(
            name: "EchodeBridge",
            dependencies: ["CEchodeFfi"],
            path: "Sources/EchodeBridge",
            swiftSettings: [
                .unsafeFlags(["-strict-concurrency=complete", "-warnings-as-errors"])
            ]
        ),
        .testTarget(
            name: "EchodeBridgeTests",
            dependencies: ["EchodeBridge"],
            path: "Tests/EchodeBridgeTests",
            swiftSettings: [
                .unsafeFlags(["-strict-concurrency=complete", "-warnings-as-errors"])
            ]
        ),
    ]
)
