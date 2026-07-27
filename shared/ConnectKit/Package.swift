// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "ConnectKit",
    platforms: [.macOS(.v13), .iOS(.v17)],
    products: [
        .library(name: "ConnectKit", targets: ["ConnectKit"])
    ],
    targets: [
        .target(
            name: "ConnectKit",
            path: "Sources/ConnectKit"
        )
    ]
)
