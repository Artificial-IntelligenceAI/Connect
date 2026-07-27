// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "Connect",
    platforms: [.iOS(.v17)],
    dependencies: [
        .package(path: "../shared/ConnectKit")
    ],
    targets: [
        .executableTarget(
            name: "Connect",
            dependencies: ["ConnectKit"],
            path: "Sources/Connect"
        )
    ]
)
