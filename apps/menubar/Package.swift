// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "Sidescreen",
    platforms: [.macOS(.v15)],
    targets: [
        // Declarations and thin C shims for the private CoreGraphics and
        // SkyLight APIs. Everything unstable lives here.
        .target(
            name: "SidescreenPrivate",
            linkerSettings: [
                .linkedFramework("CoreGraphics"),
                .linkedFramework("ApplicationServices"),
            ]
        ),
        .executableTarget(
            name: "Sidescreen",
            dependencies: ["SidescreenPrivate"],
            linkerSettings: [
                .linkedFramework("AppKit"),
                .linkedFramework("ScreenCaptureKit"),
                .linkedFramework("QuartzCore"),
            ]
        ),
    ]
)
