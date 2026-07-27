// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "MessagingApp",
    platforms: [.macOS(.v13)],
    targets: [
        .executableTarget(
            name: "MessagingApp",
            path: "Sources/MessagingApp"
        )
    ]
)
