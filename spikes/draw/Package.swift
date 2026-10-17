// swift-tools-version: 6.0
import Foundation
import PackageDescription

let packageDir = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
let rustLibraryDir = packageDir
    .deletingLastPathComponent()
    .deletingLastPathComponent()
    .appendingPathComponent("target/release")
    .path

let m4: [SwiftSetting] = [.unsafeFlags(["-target-cpu", "apple-m4", "-Xcc", "-mcpu=apple-m4"])]

let package = Package(
    name: "EchodeDraw",
    platforms: [.macOS("27.0")],
    products: [
        .executable(name: "EchodeDraw", targets: ["EchodeDraw"])
    ],
    targets: [
        .target(
            name: "CEchodeDraw",
            path: "CEchodeDraw",
            publicHeadersPath: "include",
            cSettings: [.unsafeFlags(["-mcpu=apple-m4"])],
            cxxSettings: [.unsafeFlags(["-mcpu=apple-m4"])],
            linkerSettings: [
                .linkedLibrary("echode_draw"),
                .unsafeFlags(["-L", rustLibraryDir]),
            ]
        ),
        .executableTarget(
            name: "EchodeDraw",
            dependencies: ["CEchodeDraw"],
            path: "Sources/EchodeDraw",
            swiftSettings: m4,
            linkerSettings: [
                .linkedFramework("Metal"),
                .linkedFramework("QuartzCore"),
                .linkedFramework("AppKit"),
            ]
        ),
    ]
)
