// swift-tools-version: 6.2
// Throwaway spike: YouTube IFrame player in a WKWebView. Not part of the app build.
import PackageDescription

let package = Package(
    name: "YouTubeSpike",
    platforms: [.macOS(.v26)],
    targets: [
        .executableTarget(
            name: "YouTubeSpike",
            path: "Sources/YouTubeSpike",
            linkerSettings: [.linkedFramework("WebKit"), .linkedFramework("Network")]
        )
    ]
)
