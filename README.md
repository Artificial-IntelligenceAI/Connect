# Connect

A messaging app. Cross-platform and end-to-end encrypted.

## Architecture

- **Shared core (Rust, `core/`):** the actual networking + encryption
  client — connects over WebSocket and reports events back to native UIs
  through a [UniFFI](https://mozilla.github.io/uniffi-rs/) callback
  interface (`ConnectClient` / `ConnectClientListener`). Every platform's
  UI calls into this same Rust implementation instead of reimplementing
  the protocol — see [shared/README.md](shared/README.md) for how the
  Swift/Kotlin bindings get generated.
- **Real, end-to-end encrypted 1:1 DMs and group chats**, via
  [`vodozemac`](https://github.com/matrix-org/vodozemac)'s Olm (pairwise,
  X3DH-style double ratchet). There's no implicit shared "room" — every
  conversation is either an explicit 1:1 with one other peer or an
  explicit, named, fixed-membership group chat, and both are built on the
  same per-peer Olm sessions (a group message is just the same encrypted
  payload fanned out individually to each currently-online member, not a
  shared group key). The server only ever relays ciphertext, addressed by
  peer ID — see `core/src/client.rs` for the full design and its
  documented v1 limitations (one-time keys are reused across peers rather
  than consumed once each, group membership is fixed at creation from
  currently-online peers only, and a message can arrive before its
  sender's Olm session is ready and be silently dropped rather than
  retried).
- **Identity is persisted, with trust-on-first-use key-change warnings.**
  Each `ConnectClient` is constructed with a `data_dir` (a platform-
  supplied, sandboxed writable directory); its Olm identity is pickled to
  a file there and reloaded on every subsequent launch, so a given
  install has one stable identity rather than a new one every connection.
  Every contact's identity key, keyed by display name, is remembered the
  same way -- if a name you've talked to before shows up with a
  *different* key, that's surfaced as a system-message warning in the
  chat (not a hard block) rather than silently trusted. Both files are
  plain, unencrypted JSON relying on OS-level app-sandbox permissions for
  protection, not at-rest encryption -- a hardening candidate later. See
  "End-to-end encryption" in [shared/README.md](shared/README.md) for the
  full design and known limitations (TOFU is anchored to display name,
  which anyone can type, not a stronger identity).
- **Dropped connections retry automatically with backoff.** A server
  restart, wifi hiccup, or laptop sleep/wake no longer leaves the app
  stuck showing "Connected" over a dead socket: the client reports a new
  `Reconnecting` state and retries (1s, 2s, 4s, ... capped at 30s) until
  it succeeds. A user-initiated disconnect is distinct from a drop and
  does not retry. Note a reconnect (yours or a peer's) does reset the Olm
  sessions used for DMs/group messages, so a conversation mid-drop
  re-bootstraps its session next time either side sends something rather
  than picking up mid-ratchet. See "Reconnection handling" in
  [shared/README.md](shared/README.md) for the full design, including a
  real bug caught during testing (a drop with no message in flight went
  undetected until the read side was watched too, not just writes).
- **Server (Rust, Axum, `server/`):** routes ciphertext between connected
  clients — broadcasts peer-joined/left events for discovery, and relays
  1:1 messages, group invites, and group messages point-to-point to the
  specific peer(s) they're addressed to. It knows who's online and who's
  talking to whom, but never has the keys to read any content, and has no
  notion of conversations or groups itself — clients tell it exactly who
  to deliver each ciphertext to. Runs in LAN mode today (no discovery/TLS
  yet); a hosted "real server" mode is planned, using the same protocol.
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

Connect using `127.0.0.1` / port `7878` and a display name.

**The message composer is currently disabled on all three platforms.**
Sending now requires targeting a specific peer or group
(`sendDirectMessage`/`createGroup`/`sendGroupMessage`), and the existing
single connect-then-chat screen has no UI for picking one — that's the
chat-list GUI planned next. Until then, the connect flow, roster/TOFU
system notices, and reconnection banner all still work and are worth
trying; sending a message from the UI is not. See [shared/ConnectKit's
test suite](shared/README.md#automated-tests) or `core/src/client.rs`'s
own tests to see DMs/groups actually exercised today.

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
`MessagingCore.ConnectClient` directly (bypassing all UI) — connects and
prints everything the Rust core reports back (fingerprint, TOFU notices,
join/leave), useful for confirming two instances discover each other
without fighting a GUI, or for testing on iOS via `xcrun simctl spawn
<udid> <path-to-built-binary>` since it needs no Simulator UI interaction
at all. It doesn't exercise sending a DM/group message — that needs a
live peer ID this simple listener interface has no query API for yet; see
`core/src/client.rs`'s own test suite for DM/group send+receive exercised
directly. Identity persists per `dataDirTag` (defaults to `displayName`)
under a temp directory — run twice with the same tag to confirm
persistence (same fingerprint printed both times), or with the same
`displayName` but a different tag to trigger the key-changed warning:

```bash
cd shared/ConnectKit && swift run FFISmokeTest [displayName] [host] [port] [dataDirTag]
```

## License

Apache License 2.0 — see [LICENSE](LICENSE).
