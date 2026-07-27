// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "Connect",
    platforms: [.macOS(.v13)],
    targets: [
        .executableTarget(
            name: "Connect",
            path: "Sources/Connect"
        )
    ]
)
