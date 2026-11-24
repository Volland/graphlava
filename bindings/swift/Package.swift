// swift-tools-version:5.9
// The GraphLava Swift package ([[16-api#Mobile bindings]]): a `Database`
// actor over the C FFI in `GraphLavaFFI.xcframework`. Build the framework
// first with `./build-xcframework.sh` (CI does), then `swift test` on the
// macOS host or `xcodebuild test -scheme GraphLava -destination
// 'platform=iOS Simulator,name=<device>'` on a simulator.
import PackageDescription

let package = Package(
    name: "GraphLava",
    platforms: [.iOS(.v15), .macOS(.v12)],
    products: [
        .library(name: "GraphLava", targets: ["GraphLava"]),
    ],
    targets: [
        .binaryTarget(name: "GraphLavaFFI", path: "GraphLavaFFI.xcframework"),
        .target(
            name: "GraphLava",
            dependencies: ["GraphLavaFFI"],
            resources: [.copy("PrivacyInfo.xcprivacy")]
        ),
        .testTarget(name: "GraphLavaTests", dependencies: ["GraphLava"]),
    ]
)
