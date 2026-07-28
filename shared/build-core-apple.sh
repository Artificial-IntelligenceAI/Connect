#!/usr/bin/env bash
# Builds messaging-core for Apple platforms and regenerates the Swift
# bindings + XCFramework that shared/ConnectKit depends on. Run this after
# any change under core/src, before `swift build`/`swift run` in macos/ or
# ios/ -- the generated output is gitignored, not committed.
set -euo pipefail
cd "$(dirname "$0")/.."  # repo root

CONNECTKIT_DIR="shared/ConnectKit"
BINDINGS_DIR="$CONNECTKIT_DIR/Sources/MessagingCore"
XCFRAMEWORK="$CONNECTKIT_DIR/MessagingCoreFFI.xcframework"

echo "==> Building messaging-core (release, macOS arm64)"
cargo build -p messaging-core --release --target aarch64-apple-darwin

echo "==> Building messaging-core (release, iOS Simulator arm64)"
cargo build -p messaging-core --release --target aarch64-apple-ios-sim

echo "==> Generating Swift bindings"
rm -rf "$BINDINGS_DIR"
mkdir -p "$BINDINGS_DIR"
(cd core && cargo run --quiet --features uniffi/cli --bin uniffi-bindgen -- generate \
  --library "../target/aarch64-apple-darwin/release/libmessaging_core.dylib" \
  --language swift \
  --out-dir "../$BINDINGS_DIR")

echo "==> Assembling XCFramework (macOS + iOS Simulator)"
HEADERS_DIR=$(mktemp -d)
mv "$BINDINGS_DIR/messaging_coreFFI.h" "$HEADERS_DIR/"
mv "$BINDINGS_DIR/messaging_coreFFI.modulemap" "$HEADERS_DIR/module.modulemap"

rm -rf "$XCFRAMEWORK"
xcodebuild -create-xcframework \
  -library "target/aarch64-apple-darwin/release/libmessaging_core.dylib" \
  -headers "$HEADERS_DIR" \
  -library "target/aarch64-apple-ios-sim/release/libmessaging_core.dylib" \
  -headers "$HEADERS_DIR" \
  -output "$XCFRAMEWORK"

rm -rf "$HEADERS_DIR"

echo "==> Done. $XCFRAMEWORK and $BINDINGS_DIR are up to date."
