// swift-tools-version: 6.2
import PackageDescription

let package = Package(
    name: "Tabster",
    platforms: [.macOS(.v26)],
    targets: [
        // UniFFI's C header for the Rust core. `just bindings` generates the header,
        // and `just build` puts libtabster_core.a on the linker search path.
        // A static library can't carry its own link flags, so frameworks the Rust crates
        // need but Swift doesn't already pull in are listed here.
        .target(
            name: "tabster_coreFFI",
            path: "Sources/TabsterCoreFFI",
            linkerSettings: [
                .linkedLibrary("tabster_core"),
                .linkedFramework("CoreServices"),  // notify (FSEvents)
            ]
        ),
        // Pure Swift logic with no FFI or UI dependency (e.g. the video clock), so it can be unit-tested.
        .target(
            name: "TabsterKit",
            path: "Sources/TabsterKit"
        ),
        .executableTarget(
            name: "Tabster",
            dependencies: ["tabster_coreFFI", "TabsterKit"],
            path: "Sources/Tabster"
        ),
        .testTarget(
            name: "TabsterKitTests",
            dependencies: ["TabsterKit"],
            path: "Tests/TabsterKitTests",
            resources: [.copy("Resources")]
        ),
    ]
)
