# MessagingApp

A cross-platform, end-to-end encrypted messaging app.

## Architecture

- **Shared core (Rust):** networking, message protocol, and (soon) E2EE via
  [`vodozemac`](https://github.com/matrix-org/vodozemac) (Olm/Megolm ratchet).
- **Server (Rust, Axum):** relays messages between connected clients. Runs in
  LAN mode today (plain WebSocket, no discovery/TLS yet); a hosted "real
  server" mode is planned, using the same protocol.
- **macOS / iOS / iPadOS:** Swift / SwiftUI.
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
core/    shared Rust message types/protocol
server/  LAN relay server (Axum + WebSocket)
macos/   SwiftUI macOS app (Swift Package)
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

### Prebuilt app bundle

`macos/MessagingApp.app` is a prebuilt debug bundle (arm64/Apple Silicon
only) checked in for convenience — double-click it or `open
macos/MessagingApp.app` instead of running `swift run`. It's a snapshot,
**not rebuilt automatically**: after changing anything under
`macos/Sources`, regenerate it with:

```bash
cd macos && swift build
rm -rf MessagingApp.app
mkdir -p MessagingApp.app/Contents/MacOS
cp .build/arm64-apple-macosx/debug/MessagingApp MessagingApp.app/Contents/MacOS/
```

(`Contents/Info.plist` doesn't need to change unless the bundle identifier
or version does.)

## License

Apache License 2.0 — see [LICENSE](LICENSE).
