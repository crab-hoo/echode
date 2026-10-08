// swift-tools-version: 6.0
import Foundation
import PackageDescription

let packageDir = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
let rustLibraryDir = packageDir
    .deletingLastPathComponent()
    .deletingLastPathComponent()
    .appendingPathComponent("target/release")
    .path

let package = Package(
    name: "EchodeDraw",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "EchodeDraw", targets: ["EchodeDraw"])
    ],
    targets: [
        .target(
            name: "CEchodeDraw",
            path: "CEchodeDraw",
            publicHeadersPath: "include",
            linkerSettings: [
                .linkedLibrary("echode_draw"),
                .unsafeFlags(["-L", rustLibraryDir]),
            ]
        ),
        .executableTarget(
            name: "EchodeDraw",
            dependencies: ["CEchodeDraw"],
            path: "Sources/EchodeDraw",
            linkerSettings: [
                .linkedFramework("Metal"),
                .linkedFramework("QuartzCore"),
                .linkedFramework("AppKit"),
            ]
        ),
    ]
)
