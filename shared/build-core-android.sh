#!/usr/bin/env bash
# Builds messaging-core for Android (arm64-v8a) and regenerates the Kotlin
# bindings + native library that android/app depends on. Run this after any
# change under core/src, before building the Android app -- the generated
# output is gitignored, not committed.
#
# Requires: ANDROID_NDK_HOME set, cargo-ndk installed
# (cargo install cargo-ndk), and the aarch64-linux-android rustup target.
set -euo pipefail
cd "$(dirname "$0")/.."  # repo root

: "${ANDROID_NDK_HOME:?ANDROID_NDK_HOME must be set (path to the Android NDK)}"

APP_DIR="android/app"
JNILIBS_DIR="$APP_DIR/src/main/jniLibs"
KOTLIN_OUT="$APP_DIR/generated/uniffi"

echo "==> Building messaging-core (release, Android arm64-v8a)"
cargo ndk -t arm64-v8a -o "$JNILIBS_DIR" build -p messaging-core --release

echo "==> Generating Kotlin bindings"
rm -rf "$KOTLIN_OUT"
mkdir -p "$KOTLIN_OUT"
(cd core && cargo run --quiet --features uniffi/cli --bin uniffi-bindgen -- generate \
  --library "../$JNILIBS_DIR/arm64-v8a/libmessaging_core.so" \
  --language kotlin \
  --no-format \
  --out-dir "../$KOTLIN_OUT")

echo "==> Done. $JNILIBS_DIR and $KOTLIN_OUT are up to date."
