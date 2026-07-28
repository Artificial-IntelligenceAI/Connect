# Connect

A messaging app. Cross-platform and end-to-end encrypted.

## Architecture

- **Shared core (Rust, `core/`):** the actual networking client — connects
  over WebSocket, speaks the join/message/system_notice JSON protocol, and
  reports events back to native UIs through a
  [UniFFI](https://mozilla.github.io/uniffi-rs/) callback interface
  (`ConnectClient` / `ConnectClientListener`). Every platform's UI calls
  into this same Rust implementation instead of reimplementing the
  protocol — see [shared/README.md](shared/README.md) for how the Swift
  bindings get generated. E2EE (via
  [`vodozemac`](https://github.com/matrix-org/vodozemac), Olm/Megolm) is
  not implemented yet.
- **Server (Rust, Axum, `server/`):** relays messages between connected
  clients. Runs in LAN mode today (plain WebSocket, no discovery/TLS yet);
  a hosted "real server" mode is planned, using the same protocol.
- **macOS / iOS / iPadOS:** Swift / SwiftUI. The app is named "Connect" on
  every Apple platform. `shared/ConnectKit` is a local Swift Package
  holding the actual UI (`ContentView`, `Theme`) and a thin
  `NetworkClient` wrapper around the Rust `ConnectClient`; `macos/` and
  `ios/` are both thin app shells that depend on it, so the UI and
  networking logic are each implemented exactly once, shared across both
  platforms.
- **Android:** Kotlin / Jetpack Compose (`android/`). Same pattern as
  Apple: `android/app`'s `NetworkClient.kt` implements the UniFFI
  callback interface and wraps the same Rust `ConnectClient`, cross-compiled
  for Android via `cargo-ndk`. No protocol logic is reimplemented — see
  [shared/README.md](shared/README.md).
- **Windows:** C# / WinUI 3.
- **Linux:** GTK4 (`gtk4-rs`).

**Encryption is not yet implemented.** Messages are currently sent in
plaintext over the LAN relay. Do not use this for anything sensitive yet.

## Repo layout

```
core/               shared Rust networking client + protocol (UniFFI-exported)
server/             LAN relay server (Axum + WebSocket)
shared/ConnectKit/  shared SwiftUI code + generated Rust FFI bindings (Swift Package)
macos/              macOS app shell "Connect" (Swift Package)
ios/                iOS/iPadOS app shell "Connect" (Swift Package)
android/            Android app "Connect" (Gradle + Jetpack Compose)
```

## Running locally

The Rust core has to be built and its Swift bindings generated *before*
either app will build — see [shared/README.md](shared/README.md) for
what that step does. tl;dr:

```bash
./shared/build-core-apple.sh
```

Then:

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
automatically**: after changing anything under `macos/Sources`,
`shared/ConnectKit`, or `core/`, regenerate it with:

```bash
./shared/build-core-apple.sh   # if core/ changed
cd macos && ./build-app.sh
```

`build-app.sh` also embeds `libmessaging_core.dylib` inside the bundle and
rewrites its load path to be relative to the bundle (`swift build` alone
links it by the absolute path in `target/`, which only works on the
machine that built it).

### Running the iOS app

Requires full Xcode (not just Command Line Tools) with an iOS Simulator
runtime installed. `ios/` has no `.xcodeproj` — `xcodebuild` builds
straight from `Package.swift`. Get a simulator's UDID from `xcrun simctl
list devices`, then:

```bash
./shared/build-core-apple.sh   # if core/ changed
cd ios && ./build-app.sh <simulator-udid>
```

This builds, wraps the executable into a self-contained `.app` (same
dylib-embedding treatment as the macOS script, plus an `Info.plist` with
`UIApplicationSceneManifest`, which SwiftUI's `WindowGroup` needs for
scene-based lifecycle/touch delivery), signs it, and installs it on the
given simulator. Launch it with `xcrun simctl launch <udid>
com.messagingapp.connect.ios`, or open `ios/Package.swift` directly in
Xcode and run it from there instead.

### Running the Android app

Requires the Android SDK (`platform-tools`, a platform, `emulator` + a
system image) and NDK, plus `cargo-ndk` (`cargo install cargo-ndk`) and
the `aarch64-linux-android` Rust target. Set `ANDROID_HOME` and
`ANDROID_NDK_HOME`, then:

```bash
./shared/build-core-android.sh   # cross-compiles core, generates Kotlin bindings
cd android && ./gradlew assembleDebug
```

Install and launch on a running emulator/device with `adb install -r
android/app/build/outputs/apk/debug/app-debug.apk` and `adb shell am
start -n com.messagingapp.connect/.MainActivity`. Note the Android
emulator does **not** share the host's network stack the way the iOS
Simulator does — use `10.0.2.2` instead of `127.0.0.1` to reach a relay
server running on your Mac.

### Testing the Rust<->Swift FFI directly

`shared/ConnectKit`'s `FFISmokeTest` target is a small CLI that calls
`MessagingCore.ConnectClient` directly (bypassing all UI) — connects,
sends one message, and prints everything the Rust core reports back.
Useful for verifying the FFI layer itself, or for testing on iOS via
`xcrun simctl spawn <udid> <path-to-built-binary>` since it needs no
Simulator UI interaction at all:

```bash
cd shared/ConnectKit && swift run FFISmokeTest
```

## License

Apache License 2.0 — see [LICENSE](LICENSE).
