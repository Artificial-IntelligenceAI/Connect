// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "ConnectKit",
    platforms: [.macOS(.v13), .iOS(.v17)],
    products: [
        .library(name: "ConnectKit", targets: ["ConnectKit"])
    ],
    targets: [
        .binaryTarget(
            name: "MessagingCoreFFI",
            path: "MessagingCoreFFI.xcframework"
        ),
        .target(
            name: "MessagingCore",
            dependencies: ["MessagingCoreFFI"],
            path: "Sources/MessagingCore"
        ),
        .target(
            name: "ConnectKit",
            dependencies: ["MessagingCore"],
            path: "Sources/ConnectKit"
        ),
        .executableTarget(
            name: "FFISmokeTest",
            dependencies: ["MessagingCore"],
            path: "Sources/FFISmokeTest"
        )
    ]
)
