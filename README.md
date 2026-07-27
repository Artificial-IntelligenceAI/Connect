# Connect

A messaging app. Cross-platform and end-to-end encrypted.

## Architecture

- **Shared core (Rust):** networking, message protocol, and (soon) E2EE via
  [`vodozemac`](https://github.com/matrix-org/vodozemac) (Olm/Megolm ratchet).
- **Server (Rust, Axum):** relays messages between connected clients. Runs in
  LAN mode today (plain WebSocket, no discovery/TLS yet); a hosted "real
  server" mode is planned, using the same protocol.
- **macOS / iOS / iPadOS:** Swift / SwiftUI. The app is named "Connect" on
  every Apple platform. `shared/ConnectKit` is a local Swift Package
  holding the actual UI/networking code (`ContentView`, `NetworkClient`,
  `Theme`); `macos/` and `ios/` are both thin app shells that depend on it,
  so there is exactly one implementation of the app shared between
  platforms.
- **Android:** Kotlin / Jetpack Compose.
- **Windows:** C# / WinUI 3.
- **Linux:** GTK4 (`gtk4-rs`).

Native clients are meant to share the Rust core via UniFFI bindings. The
current macOS app is a v0 that talks to the server directly over WebSocket
to get the pipeline working end-to-end; wiring it through the Rust core via
UniFFI is the next step.

**Encryption is not yet implemented.** Messages are currently sent in
plaintext over the LAN relay. Do not use this for anything sensitive yet.

## Repo layout

```
core/               shared Rust message types/protocol
server/             LAN relay server (Axum + WebSocket)
shared/ConnectKit/  shared SwiftUI code (Swift Package), used by both apps below
macos/              macOS app shell "Connect" (Swift Package)
ios/                iOS/iPadOS app shell "Connect" (Swift Package)
```

## Running locally

```bash
# Terminal 1: start the relay server
cargo run -p messaging-server

# Terminal 2: run the macOS app
cd macos && swift run
```

Connect using `127.0.0.1` / port `7878` and a display name. Run a second
instance to chat with yourself locally.

### Prebuilt macOS app bundle

`macos/Connect.app` is a prebuilt debug bundle (arm64/Apple Silicon only)
checked in for convenience — double-click it or `open macos/Connect.app`
instead of running `swift run`. It's a snapshot, **not rebuilt
automatically**: after changing anything under `macos/Sources` or
`shared/ConnectKit`, regenerate it with:

```bash
cd macos && swift build
rm -rf Connect.app
mkdir -p Connect.app/Contents/MacOS
cp .build/arm64-apple-macosx/debug/Connect Connect.app/Contents/MacOS/
```

(`Contents/Info.plist` doesn't need to change unless the bundle identifier
or version does.)

### Running the iOS app

Requires full Xcode (not just Command Line Tools) with an iOS Simulator
runtime installed. `ios/` has no `.xcodeproj` — `xcodebuild` can build
straight from `Package.swift`:

```bash
cd ios
xcodebuild -scheme Connect -destination 'platform=iOS Simulator,name=iPhone 17' build
```

That produces a bare executable (not an `.app`), since SwiftPM executable
targets aren't iOS app bundles on their own. Wrap and install it with:

```bash
BINARY=$(find ~/Library/Developer/Xcode/DerivedData/ios-*/Build/Products/Debug-iphonesimulator/Connect -type f | head -1)
rm -rf Connect.app && mkdir Connect.app
cp "$BINARY" Connect.app/Connect
cp Info.plist.template Connect.app/Info.plist   # see below
codesign --force --sign - Connect.app
xcrun simctl install booted Connect.app
xcrun simctl launch booted com.messagingapp.connect.ios
```

There's no `Info.plist.template` checked in yet -- copy the `Info.plist` an
Xcode-generated iOS app target produces (it needs `CFBundleExecutable`,
`CFBundleIdentifier`, `UILaunchScreen`, and critically
`UIApplicationSceneManifest`, which SwiftUI's `WindowGroup` relies on for
scene-based touch delivery). The easiest path if you hit trouble: open
`ios/Package.swift` directly in Xcode and run it from there instead of the
command line.

## License

Apache License 2.0 — see [LICENSE](LICENSE).
